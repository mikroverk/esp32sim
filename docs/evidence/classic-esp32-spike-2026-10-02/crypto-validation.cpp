#include <Arduino.h>
#include <mbedtls/aes.h>
#include <mbedtls/bignum.h>
#include <mbedtls/md.h>
#include <mbedtls/sha1.h>
#include <mbedtls/sha256.h>
#include <mbedtls/sha512.h>

unsigned passed, failed;

void check(const char *name, bool ok) {
  Serial.printf("%s %s\n", name, ok ? "PASS" : "FAIL");
  if (ok) ++passed; else ++failed;
}

void unhex(const char *hex, unsigned char *out) {
  for (size_t i = 0; hex[i]; i += 2) {
    char byte[] = {hex[i], hex[i + 1], 0};
    out[i / 2] = strtoul(byte, nullptr, 16);
  }
}

bool matches(const unsigned char *out, const char *hex) {
  unsigned char expected[64];
  unhex(hex, expected);
  return memcmp(out, expected, strlen(hex) / 2) == 0;
}

void digest(const char *name, unsigned bits, const unsigned char *input,
            size_t size, const char *expected) {
  unsigned char out[64];
  int rc = bits == 1 ? mbedtls_sha1(input, size, out)
      : bits == 256 ? mbedtls_sha256(input, size, out, 0)
      : mbedtls_sha512(input, size, out, bits == 384);
  check(name, rc == 0 && matches(out, expected));
}

void ecb(unsigned bits, const char *expected) {
  unsigned char key[32], plain[16], cipher[16], recovered[16];
  for (unsigned i = 0; i < sizeof(key); ++i) key[i] = i;
  unhex("00112233445566778899aabbccddeeff", plain);
  mbedtls_aes_context ctx;
  mbedtls_aes_init(&ctx);
  int rc = mbedtls_aes_setkey_enc(&ctx, key, bits);
  rc |= mbedtls_aes_crypt_ecb(&ctx, MBEDTLS_AES_ENCRYPT, plain, cipher);
  char label[40];
  snprintf(label, sizeof(label), "AES%u_ECB_ENCRYPT", bits);
  check(label, rc == 0 && matches(cipher, expected));
  rc = mbedtls_aes_setkey_dec(&ctx, key, bits);
  rc |= mbedtls_aes_crypt_ecb(&ctx, MBEDTLS_AES_DECRYPT, cipher, recovered);
  snprintf(label, sizeof(label), "AES%u_ECB_DECRYPT", bits);
  check(label, rc == 0 && memcmp(recovered, plain, sizeof(plain)) == 0);
  mbedtls_aes_free(&ctx);
}

void cbc(unsigned bits, const char *keyhex, const char *expected) {
  unsigned char key[32], iv[16], plain[64], cipher[64], recovered[64];
  unhex(keyhex, key);
  unhex("000102030405060708090a0b0c0d0e0f", iv);
  unhex("6bc1bee22e409f96e93d7e117393172a"
        "ae2d8a571e03ac9c9eb76fac45af8e51"
        "30c81c46a35ce411e5fbc1191a0a52ef"
        "f69f2445df4f9b17ad2b417be66c3710", plain);
  mbedtls_aes_context ctx;
  mbedtls_aes_init(&ctx);
  int rc = mbedtls_aes_setkey_enc(&ctx, key, bits);
  rc |= mbedtls_aes_crypt_cbc(&ctx, MBEDTLS_AES_ENCRYPT, sizeof(plain), iv, plain, cipher);
  char label[40];
  snprintf(label, sizeof(label), "AES%u_CBC_ENCRYPT", bits);
  check(label, rc == 0 && matches(cipher, expected));
  unhex("000102030405060708090a0b0c0d0e0f", iv);
  rc = mbedtls_aes_setkey_dec(&ctx, key, bits);
  rc |= mbedtls_aes_crypt_cbc(&ctx, MBEDTLS_AES_DECRYPT, sizeof(cipher), iv, cipher, recovered);
  snprintf(label, sizeof(label), "AES%u_CBC_DECRYPT", bits);
  check(label, rc == 0 && memcmp(recovered, plain, sizeof(plain)) == 0);
  mbedtls_aes_free(&ctx);
}

void mpi(const char *name, const char *ahex, const char *bhex,
         const char *modulus, const char *expected) {
  mbedtls_mpi a, b, m, z, want;
  mbedtls_mpi_init(&a); mbedtls_mpi_init(&b); mbedtls_mpi_init(&m);
  mbedtls_mpi_init(&z); mbedtls_mpi_init(&want);
  int rc = mbedtls_mpi_read_string(&a, 16, ahex);
  rc |= mbedtls_mpi_read_string(&b, 16, bhex);
  rc |= mbedtls_mpi_read_string(&want, 16, expected);
  if (modulus) {
    rc |= mbedtls_mpi_read_string(&m, 16, modulus);
    if (rc == 0) rc = mbedtls_mpi_exp_mod(&z, &a, &b, &m, nullptr);
  } else if (rc == 0) {
    rc = mbedtls_mpi_mul_mpi(&z, &a, &b);
  }
  check(name, rc == 0 && mbedtls_mpi_cmp_mpi(&z, &want) == 0);
  mbedtls_mpi_free(&a); mbedtls_mpi_free(&b); mbedtls_mpi_free(&m);
  mbedtls_mpi_free(&z); mbedtls_mpi_free(&want);
}

void setup() {
  Serial.begin(115200);
  Serial.printf("CRYPTO_BEGIN hardware_aes=%d hardware_sha=%d hardware_mpi=%d\n",
                CONFIG_MBEDTLS_HARDWARE_AES, CONFIG_MBEDTLS_HARDWARE_SHA,
                CONFIG_MBEDTLS_HARDWARE_MPI);
  const unsigned char abc[] = "abc";
  unsigned char multi[200];
  for (unsigned i = 0; i < sizeof(multi); ++i) multi[i] = i;
  digest("SHA1_ABC", 1, abc, 3, "a9993e364706816aba3e25717850c26c9cd0d89d");
  digest("SHA1_MULTI", 1, multi, sizeof(multi), "54d11e99127d159799dbce10f51a75e697780478");
  digest("SHA256_ABC", 256, abc, 3, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  digest("SHA256_MULTI", 256, multi, sizeof(multi), "1901da1c9f699b48f6b2636e65cbf73abf99d0441ef67f5c540a42f7051dec6f");
  digest("SHA384_ABC", 384, abc, 3, "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7");
  digest("SHA384_MULTI", 384, multi, sizeof(multi), "7ea4bb2534c67036f49de7beb5fe8a2478df04ff3fef40a9cd4923999a590e9912df1297217ce1a021aa2fb1013498b8");
  digest("SHA512_ABC", 512, abc, 3, "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f");
  digest("SHA512_MULTI", 512, multi, sizeof(multi), "986058e9895e2c2ab8f9e8cbdf801db12a44842a56a91d5a4e87b1fc98b293722c4664142e42c3c551ff898646268cd92b84ed230b8c94bed7798d4f27cd7465");
  unsigned char hmac[20], hmac_key[20];
  memset(hmac_key, 0x0b, sizeof(hmac_key));
  int rc = mbedtls_md_hmac(mbedtls_md_info_from_type(MBEDTLS_MD_SHA1),
      hmac_key, sizeof(hmac_key), (const unsigned char *)"Hi There", 8, hmac);
  check("HMAC_SHA1", rc == 0 && matches(hmac, "b617318655057264e28bc0b6fb378c8ef146be00"));
  ecb(128, "69c4e0d86a7b0430d8cdb78070b4c55a");
  ecb(192, "dda97ca4864cdfe06eaf70a0ec0d7191");
  ecb(256, "8ea2b7ca516745bfeafc49904b496089");
  cbc(128, "2b7e151628aed2a6abf7158809cf4f3c",
      "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2"
      "73bed6b8e3c1743b7116e69e222295163ff1caa1681fac09120eca307586e1a7");
  cbc(256, "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4",
      "f58c4c04d6e5f1ba779eabfb5f7bfbd69cfc4e967edb808d679f777bc6702c7d"
      "39f23369a9d9bacfa530e26304231461b2eb05e2c39be9fcda6c19078c6a9d1b");
  mpi("MPI_EXP_MOD_2048", "400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0", "10001", "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff61", "f4d088d14f52d2a9b5444b2fad131ea853a375f3dd8c7f1d70b74143a170771865e2a1998de1a91a9d0f3349c25f0680af2ef4f510972da3dc1051ca2f0a4ef205bfc54c01dbd18f5c50815ed700153f1f57321d1160b048654cbfd2a235a998b874fa3dbcbceaf11337c61371096d1aa47f3534ab42faf6bdf1d9a5f48d78f4ef8b87d70d7398bfcc401b63e318896e72f1350a7c89b1d83037dec822a95a710c270f901efba0273c2f946ca0a9716f7771bc0b109cda6350513290feff77274fd599674552aa559f4c474c4e4b4bed1ff24941d1ed0afed903f2c74789cb8a61e409f701d920bb6b33b921889045a4cd6420fe25ddd93d9ca632a539d23564");
  mpi("MPI_MULT_1024", "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210", "123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0", nullptr, "121fa00ad77d742247acc9140513b7447d39f21d32a9fa66b2c71b2660403d88e854442f8dd680ab1de16d38bb6cc3cd536e9641e90306ef88fbbf4b16994a11be88e854442f8d33f416115d71c5d05629a33a669f5c13785f30636fccf2569a94bd8c78fa8899bcca4ab582281edcdeffd7de8b55b5200135650794834b632346b2f08801e6be011125c77ed4507adedb989e75a6ba37bca60b756c7923f49a707e4c634b8db1783af1235a1df76e560563fa50f0612b33cfd6d147c2cae8119a49a83e9534a4ef64bc7f35679e61cd2f2f562c3a081eaaf9a22d230c71db88c4150419dedb98668e87db10b145554458fab20783af1222236d88fe5618cf00");
  mpi("MPI_MULT_2304_MOD_FALLBACK", "80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210", "123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0", nullptr, "91a2b3c4d5e6f78091a2b3c4d5e6f78091a2b3c4d5e6f78091a2b3c4d5e6f780000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000121fa00ad77d742247acc9140513b7447d39f21d32a9fa66b2c71b2660403d88d634a424b6590c88d634a424b6590c88d634a424b6590c88d634a424b6590c88d634a424b6590c88d634a424b6590c88d634a424b6590c88d634a424b6590c88d634a424b6590c88d634a424b6590c88d634a424b6590c88d634a424b6590c88c4150419dedb98668e87db10b145554458fab20783af1222236d88fe5618cf00");
  Serial.printf("CRYPTO_DONE passed=%u failed=%u\n", passed, failed);
}

void loop() { delay(1000); }
