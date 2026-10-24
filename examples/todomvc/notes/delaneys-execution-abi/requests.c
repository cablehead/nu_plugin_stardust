/* Proposed C usage. Native submit/poll/cancel are not implemented yet.
 * The caller owns an open database, polls its responses, and closes it.
 */
#include <inttypes.h>
#include <stddef.h>
#include <stdio.h>

#include "stardust.h"
#include "requests.h"

static int64_t submit_dust(
    stardust_db *db,
    const char *source,
    size_t length,
    int64_t *firstRequestId
) {
    *firstRequestId = 0;
    stardust_document *requests = NULL;
    stardust_bytes bytes = {
        .data = (const uint8_t *)source,
        .length = (int64_t)length,
    };
    int64_t status = stardust_parse(bytes, STARDUST_DUST, &requests);
    if (status == STARDUST_OK) {
        status = stardust_submit(db, requests, firstRequestId);
    }
    stardust_document_release(requests);
    return status;
}

/* firstRequestId identifies the first batch even if the later call fails.
 * Its offsets are 0=query subscription, 1=event subscription, 2=push stream.
 * The caller keeps responsibility for all accepted requests.
 */
int64_t submit_example(stardust_db *db, int64_t *firstRequestId) {
    if (firstRequestId == NULL) {
        return STARDUST_INVALID_ARGUMENT;
    }

    /* No caller-selected requestId appears in any new request. */
    const char starts[] =
        "[{type querySubscribe path todos final true}"
        " {type eventsSubscribe pattern todos.changed final true}"
        " {type eventsPush subject todos.import data {id 1}}]";
    int64_t status = submit_dust(db, starts, sizeof(starts) - 1, firstRequestId);
    if (status != STARDUST_OK) {
        return status;
    }

    /* Use the assigned block. If its first ID is 17, this stream's ID is 19. */
    int64_t pushRequestId = *firstRequestId + 2;
    char frames[256];
    int length = snprintf(
        frames,
        sizeof(frames),
        "[{requestId %" PRId64 " type eventsPush data {id 2}}"
        " {requestId %" PRId64 " type eventsPush final true}]",
        pushRequestId,
        pushRequestId
    );
    if (length < 0 || (size_t)length >= sizeof(frames)) {
        return STARDUST_FAILED;
    }

    /* Continuations use the assigned ID. They allocate no new request IDs. */
    int64_t continuationFirstId = 0;
    return submit_dust(db, frames, (size_t)length, &continuationFirstId);
}
