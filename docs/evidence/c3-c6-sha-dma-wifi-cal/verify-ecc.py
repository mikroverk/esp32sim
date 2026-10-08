#!/usr/bin/env python3
"""Reproduce the unit tests' public scalar-multiplication vectors with OpenSSL."""
import json
import subprocess

vectors = []
for bits, scalar, expected in [
    (256, 2, "7cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc47669978" "07775510db8ed040293d9ac69f7430dbba7dade63ce982299e04b79d227873d1"),
    (256, 0x123456789abcdef, "3988322ab9f52c7f11d5d1aa92a2ac0b00275bcad8e934682257323fda672482" "855b7389f116c19c0014311c3d57dc02001e3a0ec8bd90c797732034aacd9918"),
]:
    # SEC1 ECPrivateKey containing a public test scalar and a named-curve OID.
    key = scalar.to_bytes(bits // 8, "big")
    oid = bytes.fromhex("06082a8648ce3d030107")
    body = b"\x02\x01\x01\x04" + bytes([len(key)]) + key + b"\xa0" + bytes([len(oid)]) + oid
    der = b"\x30" + bytes([len(body)]) + body
    result = subprocess.run(["openssl", "pkey", "-inform", "DER", "-pubout", "-outform", "DER"],
                            input=der, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    public = result.stdout[-(bits // 4):].hex()
    assert public == expected, (bits, scalar, public)
    vectors.append({"curve": f"P-{bits}", "scalar": hex(scalar), "xy": public})
print(json.dumps({"openssl": subprocess.check_output(["openssl", "version"], text=True).strip(),
                  "vectors": vectors, "result": "PASS"}, indent=2))
