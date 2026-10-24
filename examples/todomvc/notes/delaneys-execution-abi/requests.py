"""Proposed generated binding usage. These methods are not implemented yet.

The database owner selects blocking or async behavior when opening it.
One binding dispatcher polls its native response queue. Typed subscriptions
receive separate batches without creating native queues or polling threads.
subscribe omits requestId. Native submit assigns the ID, and the subscription
stores it for subsequent operations. Application code never chooses an ID.
"""

from dataclasses import dataclass
from datetime import timedelta

from stardust import (
    AsyncDatabase,
    Database,
    EventsMessage,
    EventsSubscribeRequest,
    Failure,
    Outcome,
)


@dataclass
class Todo:
    id: int
    title: str


def print_batch(batch: list[Outcome[EventsMessage[Todo]]]) -> None:
    for response in batch:
        if isinstance(response, Failure):
            raise RuntimeError(f"{response.error.code}: {response.error.message}")
        message = response.result
        print(message.subject, message.data.id, message.data.title)


def consume_blocking(database: Database) -> None:
    # Closing the subscription cancels only its request. The database stays open.
    with database.events.subscribe(
        EventsSubscribeRequest(pattern="todos.changed"),
        payload_type=Todo,
    ) as subscription:
        while True:
            # None means this typed request ended. [] means the wait expired.
            batch = subscription.poll(max_wait=timedelta(seconds=1))
            if batch is None:
                return
            if batch:
                print_batch(batch)
                return


async def consume_async(database: AsyncDatabase) -> None:
    # The database and its dispatcher belong to the current asyncio event loop.
    async with database.events.subscribe(
        EventsSubscribeRequest(pattern="todos.changed"),
        payload_type=Todo,
    ) as subscription:
        while True:
            # This awaits routed host values. The native callback only signals
            # the dispatcher through loop.call_soon_threadsafe.
            batch = await subscription.poll(max_wait=timedelta(seconds=1))
            if batch is None:
                return
            if batch:
                print_batch(batch)
                return

# Both functions borrow an open database. Its owner must close it on shutdown,
# including when a consumer fails. Async close waits for native cleanup outside
# the event-loop thread. The async subscription context cancels and completes
# its own request without closing the database.
