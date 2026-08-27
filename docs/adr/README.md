# Architecture Decision Records

One decision per file, with the reasoning that led to it.

## Why bother

favjit leans on low-level OS input APIs, and the candidates differ sharply in what they can and cannot do. Without a record of *why* an API was picked — and why another was ruled out — the same investigation gets repeated.

Code says *how*. An ADR says *why*, and *why not*.

## Naming

```
docs/adr/NNNN-short-slug.md
```

`NNNN` is a zero-padded sequence number; the slug is ASCII kebab-case. The numbers are consecutive with no holes, so a decision that leaves this directory is followed by renumbering the ones after it. A gap would be the one thing about this set a reader cannot check — whether the missing file was decided elsewhere, abandoned, or never written — and a citation is by number, which a `grep` finds wherever it moved to.

## What is an architecture decision

**A decision about how favjit is built, not about how one platform is dealt with.** The crate layout, the host boundary, what the link carries, what makes the suite deterministic: each of those is a choice that would have been a different program if it had gone the other way, and each holds on both machines.

**"On this platform, do it this way" is not one of them.** Which OS mechanism carries the output, how the process is installed, which of the two homes a file sits under: each is settled by what one platform makes possible, so it belongs beside the findings that settle it, under [docs/platform/](../platform/). A decision like that written here reads as though favjit had a choice of platform to make it about, and it does not — there is one Windows machine and one Mac ([ADR-0002](0002-input-topology.md)).

## Status values

| Status | Meaning |
|---|---|
| `Accepted` | Decided. Implementation follows this. |
| `Superseded by ADR-NNNN` | Replaced by a later ADR |
| `Rejected` | Considered and deliberately not taken |

**An ADR says what is decided now, so a decision that changes is rewritten where it stands.** What it said before is in the repository's history, which is where a record of what was once in force belongs; a copy of it left in the tree is one more file saying something untrue about the code beside it. A new ADR is for a decision that is new, not for editing one that is not.

## Writing one

Copy [template.md](template.md).

**Write the final state of the decision, not how it was reached.** No open questions, no options still being weighed, no annotations recording that something used to be undecided. An ADR is written when there is a decision to state; anything still in flux does not get an ADR yet.

Rejected alternatives are not trial and error — they are part of why the decision is what it is, and they belong in the ADR.

Don't write platform behavior into an ADR from memory. Where a decision depends on how an OS actually behaves, establish that first and record it under [docs/platform/](../platform/), then decide on the basis of the finding.
