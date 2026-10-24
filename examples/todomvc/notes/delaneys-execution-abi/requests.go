// Proposed generated binding usage. These methods are not implemented yet.
package example

import (
	"context"
	"errors"
	"fmt"
	"io"
	"time"

	"stardust"
)

type Todo struct {
	ID    int64  `json:"id"`
	Title string `json:"title"`
}

// Consume prints one batch, then closes this subscription.
// Other goroutines can consume other typed requests on the same database.
// One binding dispatcher owns native polling. The database owner handles shutdown.
func Consume(ctx context.Context, database *stardust.Database) (returnErr error) {
	// This method submits a new frame without requestId. Native submit assigns
	// the ID, which the subscription stores internally for polling and close.
	subscription, err := stardust.SubscribeEvents[Todo](
		ctx,
		database,
		stardust.EventsSubscribeRequest{
			Pattern: "todos.changed",
		},
	)
	if err != nil {
		return err
	}
	defer func() {
		returnErr = errors.Join(returnErr, subscription.Close())
	}()

	for {
		// This waits on the Go scheduler for this request's buffered responses.
		// It does not call native poll or block one native thread per request.
		batch, err := subscription.Poll(ctx, time.Second)
		if errors.Is(err, io.EOF) {
			return nil
		}
		if err != nil {
			return err
		}
		if len(batch) == 0 {
			continue
		}

		// batch is []stardust.Outcome[stardust.EventsMessage[Todo]].
		for index := range batch {
			response := batch[index]
			if response.Error != nil {
				return response.Error
			}

			message := response.Result
			fmt.Printf("%s: %d %s\n", message.Subject, message.Data.ID, message.Data.Title)
		}

		return nil
	}
}
