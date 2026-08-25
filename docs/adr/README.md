# Architecture Decision Records

One decision per file, with the reasoning that led to it.

## Why bother

favjit leans on low-level OS input APIs, and the candidates differ sharply in what they can and cannot do. Without a record of *why* an API was picked — and why another was ruled out — the same investigation gets repeated.

Code says *how*. An ADR says *why*, and *why not*.

## Naming

```
docs/adr/NNNN-short-slug.md
```

`NNNN` is a zero-padded sequence number; the slug is ASCII kebab-case. Numbers are never reused, and abandoned ADRs keep their number rather than leaving a hole.

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
