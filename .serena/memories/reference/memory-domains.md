Serena memory loading policy for Solaris:

`AGENTS.md` is the canonical domain -> memory table. Do not maintain a second
divergent table here.

Rules:

- Always call Serena onboarding check after project activation.
- Do not blanket-read all memories at startup.
- Update memories only for facts likely to help future sessions: milestone
  state, oracle paths, validation workflow, owner preferences, or tooling setup.
- Do not store transient command output, speculative parity claims, secrets, or
  Mojang/vendor artifacts.
