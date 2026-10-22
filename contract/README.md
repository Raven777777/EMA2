# Runtime contract

`contract/runtime.json` is the single canonical source for product, guard, and
page-helper versions plus tool timeout budgets. `extension/contract.js` is
generated from it and must not be edited by hand.

## Updating

1. Edit `contract/runtime.json` only.
2. Regenerate the extension copy:

   ```bash
   python scripts/sync_contract.py
   ```

3. Verify the committed file matches the source:

   ```bash
   python scripts/sync_contract.py --check
   ```

   `--check` exits non-zero when `extension/contract.js` is missing or stale.

Commit the JSON and generated JS together. The script validates required keys,
positive numeric values, and the timeout ordering rules before writing.

## Timeout ordering invariant

Daemon budgets must expire later than the matching extension watchdog, so the
extension's clean `tool_timeout` can win instead of the daemon cutting the
connection first. The documented batch exception is the 600 s product cap
(`batchMaxSecs`), paired with a 580 s extension watchdog
(`batchExtensionWatchdogMs`) so the daemon cap still returns last.

`relayMarginSecs` is the daemon-side margin over the extension wait. For `js`,
`open`, and `wait`, it must also exceed the corresponding extension grace so
the relay does not time out before the extension does.
