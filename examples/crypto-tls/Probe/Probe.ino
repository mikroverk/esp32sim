// Deterministic test-only TLS peers. The public PSK and RNG must never be used in an application.
#include <WiFi.h>
#include "mbedtls/ssl.h"
#include "mbedtls/ecp.h"

struct Pipe { unsigned char bytes[8192]; size_t size; } pipes[2];
struct Endpoint { Pipe *tx, *rx; } endpoints[] = {{&pipes[0], &pipes[1]}, {&pipes[1], &pipes[0]}};
static int send_bytes(void *ctx, const unsigned char *data, size_t len) {
    Pipe *p = ((Endpoint *)ctx)->tx;
    if (len > sizeof(p->bytes) - p->size) return MBEDTLS_ERR_SSL_WANT_WRITE;
    memcpy(p->bytes + p->size, data, len); p->size += len;
    return len;
}
static int receive_bytes(void *ctx, unsigned char *data, size_t len) {
    Pipe *p = ((Endpoint *)ctx)->rx;
    if (!p->size) return MBEDTLS_ERR_SSL_WANT_READ;
    len = min(len, p->size);
    memcpy(data, p->bytes, len); p->size -= len;
    memmove(p->bytes, p->bytes + len, p->size);
    return len;
}
static int random_bytes(void *ctx, unsigned char *data, size_t len) {
    uint32_t *state = (uint32_t *)ctx;
    while (len--) { *state ^= *state << 13; *state ^= *state >> 17; *state ^= *state << 5; *data++ = *state; }
    return 0;
}
static void check(int rc) {
    if (rc) { printf("TLS FAIL %d\n", rc); fflush(stdout); abort(); }
}
void setup() {
    printf("TLS fixture start\n");
    WiFi.begin("esp32sim", "esp32sim-pass");
    for (int n = 0; n < 200 && WiFi.status() != WL_CONNECTED; ++n) delay(50);
    if (WiFi.status() != WL_CONNECTED) { printf("WiFi FAIL\n"); abort(); }
    printf("TLS GOT_IP %s\n", WiFi.localIP().toString().c_str());
    mbedtls_ssl_context ssl[2]; mbedtls_ssl_config config[2];
    uint32_t random[2] = {0x12345678, 0x87654321};
    const unsigned char psk[] = "esp32sim-public-fixture-key";
    const unsigned char identity[] = "esp32sim";
    const int suites[] = {MBEDTLS_TLS_ECDHE_PSK_WITH_AES_128_CBC_SHA256, 0};
    const uint16_t groups[] = {MBEDTLS_SSL_IANA_TLS_GROUP_SECP256R1, 0};
    for (int i = 0; i < 2; ++i) {
        mbedtls_ssl_init(&ssl[i]); mbedtls_ssl_config_init(&config[i]);
        check(mbedtls_ssl_config_defaults(&config[i], i ? MBEDTLS_SSL_IS_SERVER : MBEDTLS_SSL_IS_CLIENT, MBEDTLS_SSL_TRANSPORT_STREAM, MBEDTLS_SSL_PRESET_DEFAULT));
        mbedtls_ssl_conf_min_tls_version(&config[i], MBEDTLS_SSL_VERSION_TLS1_2);
        mbedtls_ssl_conf_max_tls_version(&config[i], MBEDTLS_SSL_VERSION_TLS1_2);
        mbedtls_ssl_conf_ciphersuites(&config[i], suites);
        mbedtls_ssl_conf_groups(&config[i], groups);
        mbedtls_ssl_conf_rng(&config[i], random_bytes, &random[i]);
        check(mbedtls_ssl_conf_psk(&config[i], psk, sizeof(psk) - 1, identity, sizeof(identity) - 1));
        check(mbedtls_ssl_setup(&ssl[i], &config[i]));
        mbedtls_ssl_set_bio(&ssl[i], &endpoints[i], send_bytes, receive_bytes, nullptr);
    }
    bool done[2] = {};
    for (int step = 0; step < 10000 && (!done[0] || !done[1]); ++step) {
        for (int i = 0; i < 2; ++i) if (!done[i]) {
            int rc = mbedtls_ssl_handshake(&ssl[i]);
            if (!rc) done[i] = true;
            else if (rc != MBEDTLS_ERR_SSL_WANT_READ && rc != MBEDTLS_ERR_SSL_WANT_WRITE) check(rc);
        }
    }
    if (!done[0] || !done[1]) check(-1);
    unsigned char message[256], received[256];
    for (int n = 0; n < 256; ++n) message[n] = n;
    for (int i = 0; i < 2; ++i) {
        check(mbedtls_ssl_write(&ssl[i], message, sizeof(message)) == sizeof(message) ? 0 : -2);
        check(mbedtls_ssl_read(&ssl[1-i], received, sizeof(received)) == sizeof(received) ? 0 : -3);
        check(memcmp(message, received, sizeof(message)));
    }
    printf("TLS PASS %s %s bidirectional=256\n", mbedtls_ssl_get_version(&ssl[0]), mbedtls_ssl_get_ciphersuite(&ssl[0]));
    for (int i = 0; i < 2; ++i) { mbedtls_ssl_free(&ssl[i]); mbedtls_ssl_config_free(&config[i]); }
    fflush(stdout);
}
void loop() { delay(1000); }
