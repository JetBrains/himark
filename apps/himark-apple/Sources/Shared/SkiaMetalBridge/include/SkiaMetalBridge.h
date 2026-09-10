#pragma once
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct SkiaMetal SkiaMetal;

SkiaMetal *skia_metal_create(void *ca_metal_layer, void *mtl_device);
void       skia_metal_destroy(SkiaMetal *);

void      *skia_metal_begin(SkiaMetal *, int32_t width, int32_t height);

void       skia_metal_end(SkiaMetal *);

void       skia_metal_set_sync(SkiaMetal *, bool sync);

#ifdef __cplusplus
}
#endif
