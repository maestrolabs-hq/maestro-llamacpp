# Working in maestro-llamacpp

The model router agents work under.

Written for an agent, and true for a person.

## Before anything else

Read `README.md` for what this is, the
[model router design](docs/superpowers/specs/2026-09-03-model-router-design.md)
for where it is going, and `docs/adr/` for what has already been decided. A
decision recorded there was made with reasons. Reopen it with new evidence,
not with a preference.

## What this repository is

A router that supervises `llama-server` child processes and serves one
OpenAI-compatible endpoint per model. Two consequences shape almost every
change here:

**It owns processes it did not write.** A child can hang, exit, or be killed
by the operating system. Treat every spawn, health check and shutdown as a
thing that fails, and say what happens when it does.

**It must behave the same on Windows, macOS and Linux.** Process semantics,
path separators and signal handling all differ across them. Derive paths and
resolve behaviour at run time rather than assuming the platform you are on.
The `fast / cross-platform` legs run on every pull request and are the check
that catches this.

---

## The four rules

From [Andrej Karpathy's observations](https://x.com/karpathy/status/2015883857489522876)
on how coding agents actually fail. They bias toward caution over speed; on a
one-line change, use judgement.

### 1. Think before coding

State your assumptions **before** implementing. Where more than one reading of
the request exists, present them rather than silently picking one. If a simpler
approach exists, say so and push back. If something is unclear, stop and name
what is confusing.

The failure this prevents: confidently building the wrong thing, fast.

### 2. Simplicity first

Write the least code that solves the problem. No speculative abstraction, no
configuration hook for a single caller, no error handling for a state that
cannot occur. Three similar lines beat a premature abstraction.

The failure this prevents: a framework where a function was wanted.

### 3. Surgical changes

Touch what the task requires and nothing else. A bug fix is not an invitation
to reformat the file, rename the variables, or upgrade the dependency. Drive-by
changes hide the real diff and make a revert dangerous.

The failure this prevents: a two-line fix arriving as a two-hundred-line diff.

### 4. Goal-driven execution

Define what "done" looks like before starting, in terms someone else could
check. Then run that check and report what it printed. "Tests pass" is a claim;
the output is evidence.

The failure this prevents: declaring success without ever verifying it.

---

## Rules that are enforced, not suggested

**Derive every path at run time.** The home directory from the environment,
the repository root from the working directory, model files from a configured
models root. The `no-absolute-paths` gate refuses a home directory, a drive
letter or a user profile, and deliberately allows `/usr`, `/opt`, `/etc`,
`/var` and `/tmp`, which name a platform rather than a machine. In a test, use
a synthetic root such as `/somewhere`.

**English only.** Prose and identifiers. `tests/language.rs` scans for Latin
diacritics.

**Conventional commits.** `feat:`, `fix:`, `docs:`, `refactor:`, `test:`,
`ci:`, `build:`, `chore:`, `perf:`, `revert:`. An organisation ruleset refuses
anything else, and the changelog is generated from them.

**Write the failing test first, and watch it fail.** Record what it printed. A
test that passes the moment it is written has proved nothing: it may be
testing the implementation that was just written rather than the behaviour
that was wanted.

**Report a blocked check rather than weakening it.** If a gate blocks
something correct, say so in the pull request. A gate that is quietly bypassed
is worse than no gate, because the repository still looks guarded.

## The shape of a change

`main` takes changes only through a pull request. Direct pushes are refused by
the platform, for the maintainer too.

```text
git switch -c <topic>
just check            # the same commands CI runs, not equivalents
git push -u origin <topic>
gh pr create --fill
gh pr merge --squash --delete-branch
```

## What the gates will tell you

Locally, hooks run formatting and lint at commit time and the rest before a
push. In CI the fast tier blocks the merge; the heavy tier runs weekly and
reports.

`tests/duplication.rs` uses an allowlist rather than a threshold. Adding an
entry is expected; adding one without its reason is not, and a second test
fails when an entry stops being true, so the list cannot rot into excuses.

## Things that will surprise you

**One crate, deliberately.** A seam is only real when something varies across
it. See [ADR 0001](docs/adr/0001-one-crate-until-a-seam-is-real.md) for the
condition that reopens this: a second consumer of a module, not a feeling that
a file is getting long.

**Renaming a CI job renames a required context.** The ruleset naming the old
one blocks every pull request until it is updated. They change together.

**The `llama-server` flag surface moves between releases.** The catalog
carries flags it does not interpret, so a change upstream is a catalog edit
rather than a code change.
