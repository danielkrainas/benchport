#!/usr/bin/env python3
"""Sign launcher updates (version.json) so launchers only install your builds.

    python3 tools/sign.py keygen launcher-signing.key   # once: make a key pair
    python3 tools/sign.py sign dist/version.json --key launcher-signing.key
    python3 tools/sign.py verify dist/version.json --public <base64 key>

`keygen` writes the private key (keep it secret: anyone who has it can push a
launcher update to your players) and prints the public key, which goes into
launcher.json as "updatePublicKey". `sign` writes `version.json.sig` next to
the manifest; upload it with version.json. A launcher with updatePublicKey set
refuses any version.json without a valid signature.

The private key can also come from an environment variable (--key-env, default
LAUNCHER_SIGNING_KEY), which is how the GitHub release workflow passes it.

Signatures are Ed25519 (RFC 8032) over the exact bytes of version.json, stored
as base64. Standard library only; the implementation follows the RFC's
reference code (slow, but it signs one small file).
"""

import argparse
import base64
import hashlib
import os
import secrets
import sys
from pathlib import Path

# ---------------------------------------------------------------- Ed25519 (RFC 8032, section 6)

P = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
D = -121665 * pow(121666, P - 2, P) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)


def _sha512(b):
    return hashlib.sha512(b).digest()


def _inv(x):
    return pow(x, P - 2, P)


def _add(a, b):
    A = (a[1] - a[0]) * (b[1] - b[0]) % P
    B = (a[1] + a[0]) * (b[1] + b[0]) % P
    C = 2 * a[3] * b[3] * D % P
    Dd = 2 * a[2] * b[2] % P
    E, F, G, H = B - A, Dd - C, Dd + C, B + A
    return (E * F % P, G * H % P, F * G % P, E * H % P)


def _mul(s, pt):
    q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            q = _add(q, pt)
        pt = _add(pt, pt)
        s >>= 1
    return q


def _equal(a, b):
    return (a[0] * b[2] - b[0] * a[2]) % P == 0 and (a[1] * b[2] - b[1] * a[2]) % P == 0


def _recover_x(y, sign):
    if y >= P:
        return None
    x2 = (y * y - 1) * _inv(D * y * y + 1)
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P != 0:
        x = x * SQRT_M1 % P
    if (x * x - x2) % P != 0:
        return None
    if (x & 1) != sign:
        x = P - x
    return x


_GY = 4 * _inv(5) % P
_GX = _recover_x(_GY, 0)
G = (_GX, _GY, 1, _GX * _GY % P)


def _compress(pt):
    zi = _inv(pt[2])
    x, y = pt[0] * zi % P, pt[1] * zi % P
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def _decompress(s):
    if len(s) != 32:
        return None
    y = int.from_bytes(s, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _recover_x(y, sign)
    return None if x is None else (x, y, 1, x * y % P)


def _expand(seed):
    h = _sha512(seed)
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


def _modl(b):
    return int.from_bytes(_sha512(b), "little") % L


def public_key(seed):
    return _compress(_mul(_expand(seed)[0], G))


def sign(seed, msg):
    a, prefix = _expand(seed)
    pub = _compress(_mul(a, G))
    r = _modl(prefix + msg)
    big_r = _compress(_mul(r, G))
    s = (r + _modl(big_r + pub + msg) * a) % L
    return big_r + int.to_bytes(s, 32, "little")


def verify(pub, msg, sig):
    if len(pub) != 32 or len(sig) != 64:
        return False
    a = _decompress(pub)
    r = _decompress(sig[:32])
    s = int.from_bytes(sig[32:], "little")
    if a is None or r is None or s >= L:
        return False
    h = _modl(sig[:32] + pub + msg)
    return _equal(_mul(s, G), _add(r, _mul(h, a)))


# ---------------------------------------------------------------- CLI


def _b64(b):
    return base64.b64encode(b).decode()


def _load_seed(args):
    if args.key:
        text = Path(args.key).read_text()
    else:
        text = os.environ.get(args.key_env, "")
        if not text.strip():
            sys.exit(f"No key: pass --key FILE or set ${args.key_env}")
    lines = [l.strip() for l in text.splitlines() if l.strip() and not l.strip().startswith("#")]
    try:
        seed = base64.b64decode(lines[-1], validate=True)
    except Exception:
        seed = b""
    if len(seed) != 32:
        sys.exit("That isn't a Toybox signing key (expected base64 of 32 bytes).")
    return seed


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    k = sub.add_parser("keygen", help="create a new key pair")
    k.add_argument("out", help="where to write the private key (must not exist)")

    s = sub.add_parser("sign", help="write <manifest>.sig")
    s.add_argument("manifest")

    pk = sub.add_parser("pubkey", help="print the public key for a private key")

    for p in (s, pk):
        p.add_argument("--key", help="private key file")
        p.add_argument("--key-env", default="LAUNCHER_SIGNING_KEY", help="environment variable holding the key (default: %(default)s)")

    v = sub.add_parser("verify", help="check <manifest>.sig")
    v.add_argument("manifest")
    v.add_argument("--public", required=True, help="base64 public key (launcher.json updatePublicKey)")

    a = ap.parse_args()

    if a.cmd == "keygen":
        out = Path(a.out)
        if out.exists():
            sys.exit(f"{out} already exists; not overwriting a key.")
        seed = secrets.token_bytes(32)
        fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write("# Toybox launcher update signing key. KEEP THIS SECRET.\n" + _b64(seed) + "\n")
        print(f"Private key written to {out} (keep it secret, and back it up).")
        print()
        print("Public key, for launcher.json:")
        print(f'  "updatePublicKey": "{_b64(public_key(seed))}"')
        print()
        print("For the GitHub release workflow:")
        print(f"  gh secret set LAUNCHER_SIGNING_KEY < {out}")
    elif a.cmd == "pubkey":
        print(_b64(public_key(_load_seed(a))))
    elif a.cmd == "sign":
        seed = _load_seed(a)
        path = Path(a.manifest)
        msg = path.read_bytes()
        sig = sign(seed, msg)
        assert verify(public_key(seed), msg, sig)
        out = path.with_name(path.name + ".sig")
        out.write_text(_b64(sig) + "\n")
        print(f"Signed {path.name} -> {out.name} (public key {_b64(public_key(seed))})")
    elif a.cmd == "verify":
        path = Path(a.manifest)
        sig_path = path.with_name(path.name + ".sig")
        try:
            pub = base64.b64decode(a.public, validate=True)
            sig = base64.b64decode(sig_path.read_text().strip(), validate=True)
        except Exception as e:
            sys.exit(f"Couldn't read the key or signature: {e}")
        if verify(pub, path.read_bytes(), sig):
            print(f"{path.name}: signature OK")
        else:
            sys.exit(f"{path.name}: BAD signature")


if __name__ == "__main__":
    main()
