# `base-intent-swap`

Intent-swap extension for the Base builder. Registers an `intent_submitOrder` RPC that
accepts signed 1inch Fusion orders, solves each order on arrival against committed AMM
state overlaid with the in-progress block's executed transactions (via the
`base-builder-core` build-event stream), and inserts the signed settlement transaction
into the builder's txpool for same-block inclusion.

`PoC` scope: no metrics, no order-status RPC (outcomes are logged), shadow-mode validation.

## Running (Base mainnet shadow)

The base repo has **no built-in shadow/non-proposing mode**: the builder builds blocks
only when driven by engine-API forkchoice updates with payload attributes. For shadow
validation you need an op-node (sequencer-mode, `--sequencer.stopped=false` variant
pointed ONLY at this builder, never publishing) or an FCU-replay harness driving the
engine API against a Base-mainnet-synced datadir. Set that up per your infra; the
builder side needs:

```sh
INTENT_SWAP_EOA_KEY=0x... \
INTENT_SWAP_TYCHO_API_KEY=... \
base-builder node \
  --builder.intent-swap.enabled \
  --builder.intent-swap.rpc-url https://mainnet.base.org \
  --builder.intent-swap.resolver-address 0x... \
  <usual builder/rollup flags>
```

Submit an order:

```sh
curl -s -X POST -H 'Content-Type: application/json' localhost:8545 \
  -d '{"jsonrpc":"2.0","id":1,"method":"intent_submitOrder","params":[{...FusionOrder json...}]}'
```

Watch for `fusion order accepted` → `solved: settlement inserted` (with tx hash and
profit) or `unfillable: ...` in the logs. Verify settlement success independently via
`eth_call` against the parent block (see `pending_sim`'s `validate_settle` in the
builder-integration repo). Notes: 1inch LOP v4 is at the same address on Base; the
resolver contract + EOA whitelisting must exist on Base before fills can land.

## Build notes

- `Cargo.lock` deliberately pins `tycho-common`, `tycho-simulation`, and
  `tycho-execution` at `0.305.1`. `fynd-core` `0.81.1` declares open `tycho`
  ranges (`>=0.304`), but does not actually compile against the latest
  `0.339.x`, so regenerating the lockfile (e.g. `cargo generate-lockfile`)
  will still break the build. Use `cargo check --workspace` to update the
  lockfile in-place instead.
- The `backrunner`/`builder-types` git dependencies use `ssh://` URLs, so building
  requires `GitHub` SSH access. If Cargo's built-in (libssh) transport can't reach
  your `ssh-agent`, add this to `.cargo/config.toml` (not committed) to shell out
  to the system `git` CLI instead:

  ```toml
  [net]
  git-fetch-with-cli = true
  ```
