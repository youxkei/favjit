# Windows

How Windows actually behaves in the areas favjit touches: input APIs and their observed semantics, device handling.

Decisions do not belong here — they live in [docs/adr/](../../adr/). This directory holds what the platform does, which is the input to those decisions rather than the outcome of them.

## Writing in this directory

One topic per file, named in ASCII kebab-case.

State what was observed on real hardware, and **record the Windows version it was observed on**. Input hook behavior changes between releases, so an undated finding is not much use.

**Write only what was actually investigated.** Don't record what an API is expected to do, or a shortlist of what might work — an empty directory is accurate, and a directory of plausible-sounding guesses is not, because the next reader cannot tell them apart from findings.
