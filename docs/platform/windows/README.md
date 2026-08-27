# Windows

How Windows actually behaves in the areas favjit touches: input APIs and their observed semantics, device handling.

**What favjit does about all that is here too**, beside the finding that settled it: how the tray item reaches a running favjit, how the programs are started at logon. Those are decided by what this platform makes possible rather than by how favjit is built, so an architecture decision is not what they are — [docs/adr/](../../adr/) says where that line falls, and a decision there holds on both machines.

## Writing in this directory

One topic per file, named in ASCII kebab-case. A file recording a decision says what was decided and why, with the alternatives that were ruled out, and links the findings it rests on.

State what was observed on real hardware, and **record the Windows version it was observed on**. Input hook behavior changes between releases, so an undated finding is not much use.

**Write only what was actually investigated.** Don't record what an API is expected to do, or a shortlist of what might work — an empty directory is accurate, and a directory of plausible-sounding guesses is not, because the next reader cannot tell them apart from findings.
