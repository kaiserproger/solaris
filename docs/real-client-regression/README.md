# Real-Client Regression Pack

This directory tracks reproducible real-client scenarios for M94+ evidence.
These manifests are not protocol harnesses and do not count as client evidence
until a normal vanilla 26.1.2 client or PrismLauncher-launched client executes
them and the required artifacts are recorded.

Artifacts from a run stay local under `.analysis/real-client-runs/<run_id>/`:

- `manifest.json` copied from the tracked scenario manifest.
- `client.log` from the real client.
- `server.log` from `cargo run --bin mc-server -- --config <config>`; the
  runner defaults to `example.toml`.
- `observations.json` with structured pass/fail notes for each step.
- `screenshots/` containing at least one screenshot for every scenario whose
  manifest entry has `screenshots_required: true`.
- `git.txt` with commit id and `git status --short --branch`.
- `toolchain.txt` with the autonomous preflight output.
- `automation-driver.txt` with the approved client kind, launch command, and
  forbidden-client guard outcome.

Protocol bots, `wire-probe`, and `mc_test_harness::client::Client` runs may be
linked as supporting harness evidence, but they must not be recorded as the
`client_gate` for these manifests.

## Runner

The approved local entrypoint is `tools/run-real-client-regression.sh`:

```sh
SOLARIS_REAL_CLIENT_KIND=prism-launcher \
SOLARIS_REAL_CLIENT_COMMAND='<real PrismLauncher or vanilla client command>' \
  bash tools/run-real-client-regression.sh --check

bash tools/run-real-client-regression.sh --prepare

SOLARIS_REAL_CLIENT_KIND=prism-launcher \
SOLARIS_REAL_CLIENT_COMMAND='<real PrismLauncher or vanilla client command>' \
  bash tools/run-real-client-regression.sh --run
```

Set `SOLARIS_REAL_CLIENT_SERVER_CONFIG=<path>` to use an explicit local config,
for example a copy of `example.toml` with `data.vanilla_data_dir` enabled.

`--run` starts Solaris, executes the configured real-client command, and writes
the local artifact directory. It does not mark scenarios passed. After a real
client executes the pack, fill `observations.json` with `client_gate` set to
`agent-run real-client`, attach screenshots/logs, and run:

```sh
bash tools/run-real-client-regression.sh --validate-run .analysis/real-client-runs/<run_id>
```

Validation checks the artifact shape and fails while the observations remain
`not-run`/prepared. The runner rejects protocol bot/mock commands before
`--run`; `--validate-run` does not re-authenticate a manually edited artifact
directory.

## Current Pack

- [`manifests/m94-regression-pack.json`](manifests/m94-regression-pack.json) is
  the bounded M94 scaffold. It covers the M94 checklist with focused scenarios
  and marks each one `manual-pending` until a real client run exists. The runner
  provides an approved automation entrypoint, not completed evidence by itself.
- `scoped_rows_manual_pending` records scoped rows that are not runnable as part
  of the bounded scenario pack yet; they stay non-green until a later real-client
  or owner-accepted degraded gate supplies artifacts.
