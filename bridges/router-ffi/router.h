/* Declares protocol 1; returned JSON buffers belong to the library until mmr_free. */
#ifndef MULTIMODAL_ROUTER_H
#define MULTIMODAL_ROUTER_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
int32_t mmr_protocol_version(void);
int32_t mmr_api_version(void);
/* input must be a live NUL-terminated UTF-8 buffer for the duration of the call. */
char *mmr_request(int64_t handle, const char *input);
/* Call exactly once for each non-null response from this same loaded library. */
void mmr_free(char *response);
#ifdef __cplusplus
}
#endif
#endif
