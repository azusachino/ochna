# Netty PR 16959 Experiment

This experiment tested `ochna` on a Java corpus, which is the primary app-analysis use case. Netty was the strongest result of the three recent corpus trials: Java class-qualified symbols made the graph precise, and the changed helper had a distinctive name that resolved cleanly.

## Subject

- Repo: `netty/netty`
- PR: `16959`
- Local submodule commit: `ec4efdbbeebf024b64e0fb782184989835c9ab92`
- Title: `Correctly release and fail queued traffic-shaping writes on close`
- Base branch: `4.2`
- Merged at: `2026-06-18T16:09:27Z`

The PR fixed queued traffic-shaping writes leaking reference-counted messages and leaving write promises incomplete when a channel/handler is closed before delayed writes flush.

## Index State

The local Netty submodule already had an ochna index:

- `3561` files
- `41087` nodes
- `104893` edges

The submodule was shallow to one commit:

```bash
git rev-list --parents -n 1 HEAD
# ec4efdbbeebf024b64e0fb782184989835c9ab92
```

The commit subject carried the PR number, so GitHub PR metadata was available:

```bash
gh api repos/netty/netty/pulls/16959
gh api repos/netty/netty/pulls/16959/files --paginate
```

## What Changed

The PR touched five files:

- `handler/src/main/java/io/netty/handler/traffic/AbstractTrafficShapingHandler.java`: `+9/-0`
- `handler/src/main/java/io/netty/handler/traffic/ChannelTrafficShapingHandler.java`: `+5/-5`
- `handler/src/main/java/io/netty/handler/traffic/GlobalChannelTrafficShapingHandler.java`: `+5/-5`
- `handler/src/main/java/io/netty/handler/traffic/GlobalTrafficShapingHandler.java`: `+5/-5`
- `handler/src/test/java/io/netty/handler/traffic/TrafficShapingHandlerTest.java`: `+49/-0`

The bug:

- `AbstractTrafficShapingHandler#calculateSize` supports `ByteBuf`, `ByteBufHolder`, and `FileRegion`.
- Traffic-shaping handlers can therefore queue delayed `ByteBufHolder` writes, such as HTTP content.
- On close or handler removal, the concrete handlers previously released only direct `ByteBuf` messages.
- Queued `ByteBufHolder` or other `ReferenceCounted` messages could leak.
- The associated `ChannelPromise`s were left incomplete even though the messages would never be written.

The fix added a shared helper:

```java
static void releaseAndFailQueuedWrite(Object msg, ChannelPromise promise, Throwable cause) {
    ReferenceCountUtil.safeRelease(msg);
    promise.tryFailure(cause);
}
```

The three concrete handlers now use that helper from `handlerRemoved`:

- `ChannelTrafficShapingHandler::handlerRemoved`
- `GlobalTrafficShapingHandler::handlerRemoved`
- `GlobalChannelTrafficShapingHandler::handlerRemoved`

Each cleanup path now:

- creates a `ClosedChannelException`
- safely releases queued messages through `ReferenceCountUtil.safeRelease`
- fails the queued write promise
- resets queue-size accounting
- clears the queue

The regression test `TrafficShapingHandlerTest::testQueuedWritesReleasedAndFailedOnClose` covers all three handler variants by writing a delayed `DefaultByteBufHolder` into an `EmbeddedChannel`, closing the channel, then asserting:

- the holder reference count drops to `0`
- the promise is done
- the failure cause is a `ClosedChannelException`
- no outbound data remains

## Ochna Workflow

Useful commands:

```bash
ochna status --json
ochna node --file handler/src/main/java/io/netty/handler/traffic/AbstractTrafficShapingHandler.java --symbols-only --json
ochna node --file handler/src/test/java/io/netty/handler/traffic/TrafficShapingHandlerTest.java --symbols-only --json
ochna node --symbol releaseAndFailQueuedWrite --include-code --json
ochna callers releaseAndFailQueuedWrite --json
ochna node --symbol testQueuedWritesReleasedAndFailedOnClose --include-code --json
```

High-signal ochna results:

- `releaseAndFailQueuedWrite` was found at `handler/src/main/java/io/netty/handler/traffic/AbstractTrafficShapingHandler.java:586`.
- `callers releaseAndFailQueuedWrite` returned exactly:
  - `ChannelTrafficShapingHandler::handlerRemoved`
  - `GlobalTrafficShapingHandler::handlerRemoved`
  - `GlobalChannelTrafficShapingHandler::handlerRemoved`
- `testQueuedWritesReleasedAndFailedOnClose` was found at `handler/src/test/java/io/netty/handler/traffic/TrafficShapingHandlerTest.java:84`.

## What Worked

This is the positive model for ochna:

- Java class-qualified methods gave useful ownership context.
- The helper name was distinctive enough that name-based lookup was precise.
- `callers` directly explained the blast radius across the three concrete traffic-shaping handlers.
- `node --symbol --include-code` gave enough implementation context without opening full files.

For Java application analysis, this is already a strong workflow: PR metadata identifies changed files, and ochna maps those files to classes, methods, helpers, callers, and tests.

## What Did Not Work

The experiment did not stress overloaded/common Java names. Netty still has many methods named `run`, `release`, `write`, `handlerRemoved`, and `initChannel`. Those can fan out if queried directly by simple name.

The lesson is not that Java is solved. The lesson is that Java is close enough that cheap receiver/import/class context should produce a large quality improvement without needing a full Java compiler.

## Plan Adjustment

For Java/Netty-style work:

1. Use `gh api` for PR metadata and changed files because the submodule is shallow.
2. Use `ochna node --file ... --symbols-only --json` to identify changed classes and methods.
3. Prefer distinctive helper/test symbols for graph traversal.
4. Use `ochna callers <symbol> --json` when the symbol is class-qualified or distinctive.
5. Be careful with common names until confidence-aware resolution lands.

## Product Lesson

Netty shows why the call-resolution redesign should start with Java. The current graph already performs well when names are distinctive and ownership is clear. Adding package/import/receiver context and resolution confidence should make common Java method queries much more trustworthy while preserving the lightweight AST-first architecture.
