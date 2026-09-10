#include <emscripten/fetch.h>
#include <stdlib.h>
#include <string.h>

unsigned char *himark_fetch_sync(const char *url, int *out_len) {
    emscripten_fetch_attr_t attr;
    emscripten_fetch_attr_init(&attr);
    strcpy(attr.requestMethod, "GET");
    attr.attributes = EMSCRIPTEN_FETCH_LOAD_TO_MEMORY | EMSCRIPTEN_FETCH_SYNCHRONOUS;
    emscripten_fetch_t *fetch = emscripten_fetch(&attr, url);
    *out_len = -1;
    if (!fetch) {
        return NULL;
    }
    if (fetch->status < 200 || fetch->status >= 300 || fetch->numBytes > 0x7fffffffull) {
        emscripten_fetch_close(fetch);
        return NULL;
    }
    int len = (int)fetch->numBytes;
    unsigned char *copy = malloc(len > 0 ? (size_t)len : 1);
    if (!copy) {
        emscripten_fetch_close(fetch);
        return NULL;
    }
    memcpy(copy, fetch->data, (size_t)len);
    emscripten_fetch_close(fetch);
    *out_len = len;
    return copy;
}
