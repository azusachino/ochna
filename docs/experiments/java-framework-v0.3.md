# Java framework relationship acceptance

Date: 2026-07-29

Both pinned submodules were archived into temporary directories and indexed
with the installed `ochna` binary. No submodule index or source file was
created or modified.

## Spring Petclinic

Pin: `b3ee2c53e76e9267f03551a7cd36b0983c859c56`

- 47 source files
- 1,344 raw calls
- 274 resolved edges
- 1,020 unresolved references
- 0.42 seconds installed-CLI indexing time

The `OwnerRepository` impact contains three `injected_into` edges to
`OwnerController`, `PetController`, and `VisitController`, each labelled
`framework_convention` with confidence 70. The
`GET /owners/{ownerId}` route is a source-anchored synthetic node with a
`route_handler` edge to `OwnerController::showOwner`, labelled
`framework_annotation` with confidence 85.

## Netty

Pin: `ec4efdbbeebf024b64e0fb782184989835c9ab92`

- 3,561 source files
- 189,470 raw calls
- 75,838 resolved edges
- 84,732 unresolved references
- 3.99 seconds installed-CLI indexing time

`releaseAndFailQueuedWrite` retains exactly the three expected
`handlerRemoved` callers, all package-resolved at confidence 80:

- `ChannelTrafficShapingHandler::handlerRemoved`
- `GlobalChannelTrafficShapingHandler::handlerRemoved`
- `GlobalTrafficShapingHandler::handlerRemoved`

`tests-for releaseAndFailQueuedWrite` does not find
`testQueuedWritesReleasedAndFailedOnClose`: the test exercises the behavior
through `testQueuedWritesReleasedAndFailedOnClose0` and handler lifecycle
rather than directly naming or calling the private helper. This is recorded as
a corpus-acceptance gap for the final v0.3 review rather than inflated into a
proven test relationship.
