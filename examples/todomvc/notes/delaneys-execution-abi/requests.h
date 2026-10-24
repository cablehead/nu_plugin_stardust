#ifndef STARDUST_REQUESTS_PROPOSAL_H
#define STARDUST_REQUESTS_PROPOSAL_H

#include <stdint.h>

/* Existing opaque types. The database owns one response queue. */
typedef struct stardust_db stardust_db;
typedef struct stardust_document stardust_document;
typedef void (*stardust_wake)(void *ctx);

#ifdef __cplusplus
extern "C" {
#endif

/* Open options gain responses, wake, and wake_ctx. See README.md. */

/* Submit an array of frames. Admission accepts all frames or none. */
/* New requests omit requestId and receive one contiguous ID block. */
/* firstRequestId is zero on failure or when no new requests are present. */
int64_t stardust_submit(
    stardust_db *db,
    const stardust_document *requests,
    int64_t *firstRequestId
);

/* One dispatcher polls. OK transfers an array document, possibly empty. */
int64_t stardust_poll(
    stardust_db *db,
    int64_t max_wait_ticks,
    stardust_document **output
);

/* Any application thread may cancel. Collect its final response through poll. */
int64_t stardust_cancel(
    stardust_db *db,
    int64_t requestId
);

#ifdef __cplusplus
}
#endif

#endif
