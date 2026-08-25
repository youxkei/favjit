# ADR-0003: Run all layout conversion through a single pipeline on the macOS side

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

Three kinds of key events need layout conversion on the Mac:

1. Events from the Mac's built-in keyboard
2. Events from Bluetooth keyboards connected to the Mac
3. Events forwarded from the Windows machine ([ADR-0002](0002-input-topology.md))

The conversion could plausibly happen in more than one place. Windows-originated events could be rewritten on the Windows side before transmission, while local events are rewritten on the Mac. That would put the same class of rules in two codebases, on two platforms, with two configuration files.

The rules themselves are the valuable part of this project — they are what the user actually tunes over time.

## Decision

One conversion pipeline, on the macOS side, downstream of every input source. The Windows source forwards raw events without interpreting them; all three sources converge on the same pipeline before injection.

## Consequences

- Conversion rules live in exactly one place, expressed once, configured once.
- The Windows binary stays deliberately thin: capture and forward. This keeps most of the project's logic on one platform, which is also the platform that can be developed and debugged against local keyboards without a second machine.
- A normalized internal event representation is required, since Windows-native and macOS-native event forms both have to map onto it losslessly enough for the rules to be written against one vocabulary. Getting this representation wrong is the main risk in this decision.
- The pipeline must be able to tell its sources apart, because per-device rules are a stated requirement: the built-in keyboard and an external Bluetooth keyboard may want different treatment. This makes the decision dependent on the capture API exposing the originating device. Where that information is unavailable, the single pipeline still stands but per-device rules are not expressible.

## Alternatives considered

### Convert on the Windows side before forwarding

Not taken. It splits the rule engine across two platforms while still requiring a full pipeline on the Mac for local keyboards — so the cost is paid twice and the benefit is zero. It also means a rule change may require touching both machines.

### A separate pipeline and rule set per source

Not taken as the primary structure: it duplicates rules that are mostly identical across sources, and divergence between copies is the predictable failure.

Note this is not the same as *per-device rules within one pipeline*, which is wanted. The distinction is one engine with device-aware rules versus several independent engines.
