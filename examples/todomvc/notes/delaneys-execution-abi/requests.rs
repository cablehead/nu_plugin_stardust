//! Proposed generated binding usage. These methods are not implemented yet.
//! One database dispatcher polls native responses and routes typed batches.
//! subscribe omits requestId. Native submit assigns the ID, and the typed
//! subscription stores it for subsequent operations. Callers never choose IDs.

use std::{error::Error, time::Duration};

use serde::Deserialize;
use stardust::{AsyncDatabase, Database, EventsMessage, EventsSubscribeRequest, Outcome};

type ExampleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Deserialize)]
struct Todo {
    id: i64,
    title: String,
}

fn print_batch(batch: Vec<Outcome<EventsMessage<Todo>>>) -> ExampleResult<()> {
    for response in batch {
        match response {
            Outcome::Success { result, .. } => {
                println!(
                    "{}: {} {}",
                    result.subject, result.data.id, result.data.title
                );
            }
            Outcome::Failure { error, .. } => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn consume_blocking(database: &Database) -> ExampleResult<()> {
    let mut subscription = database
        .events()
        .subscribe::<Todo>(EventsSubscribeRequest {
            pattern: "todos.changed".into(),
        })?;

    loop {
        // This waits on routed responses, not on a separate native queue.
        let Some(batch) = subscription.poll(Duration::from_secs(1))? else {
            return Ok(());
        };
        if batch.is_empty() {
            continue;
        }
        print_batch(batch)?;
        subscription.close()?;
        return Ok(());
    }
    // Drop requests cancellation on an early return. The dispatcher keeps the
    // request state until its final response arrives. The database stays open.
}

pub async fn consume_async(database: &AsyncDatabase) -> ExampleResult<()> {
    let mut subscription = database
        .events()
        .subscribe::<Todo>(EventsSubscribeRequest {
            pattern: "todos.changed".into(),
        })
        .await?;

    let result: ExampleResult<()> = async {
        loop {
            // This future waits for typed batches routed by the dispatcher.
            // Idle subscriptions reserve no native or blocking-helper thread.
            let Some(batch) = subscription.poll(Duration::from_secs(1)).await? else {
                return Ok(());
            };
            if batch.is_empty() {
                continue;
            }
            return print_batch(batch);
        }
    }
    .await;

    // Close waits for this request's final response. It does not close the DB.
    let cleanup = subscription.close().await;
    result?;
    cleanup?;
    Ok(())
}

// The caller owns database shutdown. Async database close performs blocking
// native cleanup outside runtime workers. Canceling this consumer future drops
// its subscription and requests cancellation, while the database dispatcher
// remains responsible for collecting the final response.
