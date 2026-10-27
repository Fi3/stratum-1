# roles_logic_sv2

Standalone Rust library for Stratum V2 pool, proxy, and miner role logic. Version 4 uses native SRI v1.12.0 protocol types. Target calculation, extranonce allocation, channel restoration, job variants, mining rounds, and fanout come from `demand/main` (`f037e6db3568454cdf7973aa6f457dd41d0856e1`).

Optional CHSEQ channel outputs and the associated channel lifecycle APIs are ported from `bona/2026-09-21-extranonce-cb-output` (`d95c79a6`) using native owned SRI messages.

This repository contains the library, tests, examples, and comparison fixtures. Applications, sockets, transaction caches, token authorization, and deployment belong to the consuming applications.

## Dependencies and imports

The published protocol dependencies are pinned to the [SRI v1.12.0 release](https://github.com/stratum-mining/stratum/releases/tag/v1.12.0):

| Crate | Version |
| --- | --- |
| `binary_sv2` | 7.0.0 |
| `common_messages_sv2` | 9.0.0 |
| `mining_sv2` | 12.0.0 |
| `job_declaration_sv2` | 10.0.0 |
| `template_distribution_sv2` | 7.0.0 |
| `framing_sv2` | 8.0.0 |
| `codec_sv2` | 7.0.0 |

`bitcoin` is a direct dependency. All eight crates are also reexported by `roles_logic_sv2`. Import wire messages from the protocol crates, and domain helpers from `roles_logic_sv2::mining::{Target, Extranonce, ExtendedExtranonce}`. `rand` is a runtime dependency for `OsRng`, which generates the factory prefix when CHSEQ is enabled. There are no old-workspace path dependencies or compatibility message definitions.

```rust
use mining_sv2::OpenStandardMiningChannelOwned;
use roles_logic_sv2::parsers::MiningOwned;
use framing_sv2::framing::{EncodableFrame, MessageFrame};

let request = OpenStandardMiningChannelOwned {
    request_id: 77,
    user_identity: b"worker.1".to_vec().try_into().unwrap(),
    nominal_hash_rate: 1_000_000.,
    max_target: [255; 32].into(),
};
let frame: MessageFrame<_> = MiningOwned::OpenStandardMiningChannel(request)
    .try_into().unwrap();
let mut wire = vec![0; frame.encoded_length()];
frame.encode_into(&mut wire).unwrap();
```

Run the complete example with `cargo run --example native_api`.

## Ownership and parsing

`parsers::{CommonMessages, Mining, JobDeclaration, TemplateDistribution, PoolMessages, MiningDeviceMessages}` contain native borrowed messages for immediate handling. Their `*Owned` counterparts contain native owned messages for caches, queued replies, and handler return values. Conversion delegates to the upstream binary library.

```rust
use roles_logic_sv2::parsers::{Mining, MiningOwned};

fn retain(message_type: u8, payload: &mut [u8])
    -> Result<MiningOwned, roles_logic_sv2::Error>
{
    let message = Mining::try_from((message_type, payload))?;
    // Inspect borrowed fields here. Copy their contents only if retaining the message.
    Ok(message.into_owned())
}
```

Stored jobs use `NewExtendedMiningJobOwned`; templates use `NewTemplateOwned`. Binary fields use types such as `U256Owned`, `B064KOwned`, and `Seq0255Owned<U256Owned>`. Replace `into_static()`/`as_static()` with native `into_owned()`/`as_owned()`, and `inner_as_ref()` with `as_bytes()`. Native owned messages do not provide the former borrowed/static facade.

Parsers reject truncated messages, unknown IDs, and trailing payload bytes. `IsSv2Message` and the message-type enums obtain IDs and channel bits from the protocol crates. Tests cover every supported message with explicit wire fixtures and verify owned values after receive-buffer reuse.

## Routing and frames

Use native `MessageFrame<T>` for messages requiring encoding and `SerializedFrame<B>` for received bytes and forwarding. There are no legacy frame wrappers.

Native standard-channel request IDs are scalar `u32` values. Mutating a decoded ID alone leaves the receive bytes unchanged. Mining handlers provide two forwarding entrypoints:

- `handle_frame_mining` validates the native header and payload, remaps the decoded request or reply, and patches the serialized ID before returning `RelaySameMessageToRemote`. `handle_message_mining` provides the same behavior for an already separated message type and payload.
- `handle_message_mining_deserialized` returns `RelayNewMessageToRemote` with an owned native message when remapping a relay. Encode this returned message instead of forwarding the original bytes.

Both paths handle requests, successes, and open-channel errors. Two downstreams can reuse the same request ID, and replies can arrive in any order. Standard-channel rejection callbacks receive the selected downstream just like success callbacks. Extended opens are managed by the caller; their rejection callbacks receive `remote: None` and the unchanged request ID, including on connections supporting both channel kinds. `Respond`, `RelayNewMessage`, and `Multiple` carry owned messages; `RelaySameMessageToRemote` refers to the checked receive frame.

Custom routers can inherit `MiningRouter::on_open_channel_error` for caller-managed extended opens. Routers that implement it return `RequestIdNotMapped` with the unchanged request ID when no standard request is registered, without consuming routing state. Other errors remain routing failures.

For callers that manage remapping themselves, `frames::RequestIdUpdate::apply` checks the native frame metadata, exact payload shape, original ID, and fixed field position before writing four bytes. `apply_payload` operates on a separated payload. The update allocates nothing and does not invoke a message encoder.

Native codec results may permit another decode without reading. Handle `Decoded::Incomplete(0)` by decoding again; after a complete frame, a zero `writable_len()` also means buffered bytes may permit progress. Fill the entire native `writable()` slice before decoding. Fragmented and consecutive frames are covered in `tests/framing.rs`. These are library tests, with no application transport loop.

`binary_sv2::to_writer()` returns the number of bytes written. Send only `&destination[..written]`, even if the destination is larger than the payload.

## Protocol changes for consumers

| Area | Native API and behavior |
| --- | --- |
| Extended groups | `OpenExtendedMiningChannelSuccessOwned` retains `group_channel_id`. Supply this native assignment to `ProxyExtendedChannelFactory::new` or `PoolChannelFactory::restore_extended_channel`. Proxy work accepts the assigned channel or group. Selectors, group channels, and proxy factories apply native `SetGroupChannel` membership changes. |
| Coinbase budgets | `CoinbaseOutputConstraints` has both `coinbase_output_max_additional_size` and `coinbase_output_max_additional_sigops`. The TD callback is `handle_coinbase_output_constraints`. |
| Token allocation | Native `AllocateMiningJobTokenSuccess` carries `request_id`, `mining_job_token`, and `coinbase_outputs`. The former async permission and additional-size fields are absent. |
| JD modes | `job_declaration::negotiated_mode(client_flags, server_requirements)` exposes `CoinbaseOnly` or `FullTemplate`. Client bit 0 allows full-template operation; server configuration bit 0 requires it. Unsupported requirements return `None`. The helper does not reinterpret `SetupConnectionSuccess` as an old async negotiation result. |
| Declaration identifiers | `DeclareMiningJob::wtxid_list` uses full transaction witness IDs. `transaction_wtxids(&transactions)` computes them in order. Coinbase fields are `coinbase_tx_prefix` and `coinbase_tx_suffix`. Block-header Merkle roots continue to use txids. |
| Solutions | JD uses `PushSolution` and `handle_push_solution`. TD retains its distinct native `SubmitSolution`. `Block::try_from(BlockCreator::new(...))` validates declaration identifiers and reconstructs the block, returning malformed-data errors. |
| Reconnect | `common_messages_sv2::Reconnect` is in `CommonMessages`; upstream common handlers expose `handle_reconnect`. It is absent from `Mining`. |
| Version rolling | Share validation applies BIP323 bits 5–28 and the extended job's `version_rolling_allowed` setting. Standard jobs allow these bits. Errors use native mining error codes. |

`SetCustomMiningJob` no longer has reward or extranonce-size fields. Its `coinbase_tx_outputs` contains the complete Bitcoin output vector: CompactSize count followed by every serialized `TxOut`. `job_creator::decode_outputs` requires full consumption, and custom-job reconstruction preserves every amount and output order. Native token outputs use the same count-prefixed format.

TD templates differ: `NewTemplate::coinbase_tx_outputs_count` is separate from its raw serialized output records. `decode_template_outputs` validates that count and consumes all records. `NewTemplate::coinbase_tx_value_remaining` still determines the amount of the first pool output; template-supplied outputs follow the pool outputs.

`PoolChannelFactory::on_new_set_custom_mining_job` returns `Result<SetCustomMiningJobSuccess, Error>` after validating the outputs and coinbase prefix. Custom shares must reference an acknowledged `(channel_id, job_id)` pair. Each channel retains at most 32 custom jobs (`MAX_CUSTOM_JOBS_PER_CHANNEL`), evicting the oldest on registration. TP tip updates discard jobs whose `prev_hash` differs from the incoming tip, preserving matching jobs registered ahead of that notification. Evicted job IDs return `ShareDoNotMatchAnyJob`; the channel remains in custom-job mode until close, which discards its cache. The full extranonce length comes from the registered native channel's prefix and `extranonce_size`, accessible through `full_extranonce_len(channel_id)`. Pool additional script data is stored separately from its allocator-managed nonce prefix and is inserted separately during coinbase reconstruction. Native restoration accepts the advertised prefix, including that data, and reserves the restored ID in the shared allocator without consuming lower available IDs. The caller authorizes tokens before accepting custom jobs. `PartialSetCustomMiningJob` is local job-building state; its `future_job` field is scheduling information, not a native custom-job wire field.

Malformed BIP34 height pushes and out-of-bounds coinbase scripts return errors. Template IDs, including `u64::MAX`, are retained without sentinel arithmetic. `replicate_upstream_extended_channel_only_jd` now accepts a native owned channel assignment and returns `Result<(), Error>`. This mock-only path retains its group and assigned nonce size and reserves its channel/group IDs, but does not reserve its extranonce space; its documented mock-pool restrictions still apply. Prefer `restore_extended_channel` for normal restoration.

## Optional channel coinbase outputs

Call `PoolChannelFactory::with_channel_coinbase_outputs()` before opening or restoring extended channels. The feature is disabled by default. Each successful open or restoration gets a stable, zero-valued `OP_RETURN` output containing `CHSEQ`, a random 16-byte factory prefix, and an 8-byte big-endian counter starting at 1. Repeated opt-in preserves that prefix and counter; a new factory gets a new prefix. `channel_coinbase_output(channel_id)` returns a copy of the assigned `ChannelCoinbaseOutput`.

Jobs returned during open/restoration, ordinary template fanout, current-job replay, and share reconstruction include the assigned marker. The shared job returned by `on_new_template_for_extended_fanout` needs materialization for each channel before sending it. Apply the channel marker first, append any local `TxOut`, then register that local output for share validation:

```rust
let (shared_job, channel_ids, standard_messages) =
    factory.on_new_template_for_extended_fanout(&mut template)?;
for channel_id in channel_ids {
    let mut job = shared_job.clone();
    job.channel_id = channel_id;
    if let Some(marker) = factory.channel_coinbase_output(channel_id) {
        marker.apply(&mut job)?;
    }
    // Append local_output to the Bitcoin output vector, preserving witness and locktime.
    // See examples/channel_coinbase_outputs.rs for the complete native transaction code.
    append_local_output(&mut job, &full_extranonce, local_output.clone())?;
    factory.register_extended_job_coinbase_output(channel_id, job.job_id, local_output);
    // Send the materialized job and the appropriate SetNewPrevHash to this channel.
}
```

Copy markers while holding the application's factory lock, then clone and materialize fanout jobs outside it. `ChannelCoinbaseOutput::apply` replaces the first reserved marker in place and removes duplicates; repeated application is idempotent. It preserves other outputs, the input sequence, the extranonce split, witness, and locktime, and returns errors for malformed or oversized suffixes without changing the job. Budget `ChannelCoinbaseOutput::additional_coinbase_weight()` for each marked coinbase: 168 weight units, including the possible two-byte CompactSize count growth. Budget local outputs separately and convert units as required by the consuming budget API.

`register_extended_job_variant(channel_id, job_id, suffix)` retains the full suffix-variant API, including arbitrary valid output sets, input sequence, witness, and locktime. During share validation the factory installs that suffix, normalizes its CHSEQ marker, and then appends the optional output registered by `register_extended_job_coinbase_output`. The latter stores one explicit `TxOut` per channel/job pair. Send miners the same final transaction; if both APIs are used, omit that appended output from the registered base suffix so it appears once. Current-job replay returns the marked canonical job; callers must reapply their local variants and outputs when replaying those changes.

Custom jobs for a marked channel must include exactly one matching CHSEQ output in their complete count-prefixed `coinbase_tx_outputs`. Use `marker.tx_out()` when assembling that vector. `try_on_new_set_custom_mining_job` returns either native `SetCustomMiningJobSuccess` or `SetCustomMiningJobErrorOwned`, preserving the channel and request IDs. `on_new_set_custom_mining_job` remains available for library errors. Token authorization belongs to the caller. The 32-job limit, eviction on tip changes, and validation by acknowledged `(channel_id, job_id)` remain in effect.

Channel and group IDs share one allocator. Sparse restored IDs reserve their exact values while lower unused IDs remain available; restored native group assignments are retained. `register_channel_user` associates an identifier with an open extended channel, rejects conflicts, and `channel_id_for_user` finds it until close. Closing a channel clears its identity, marker, and retained work. `close_group` closes grouped standard channels and releases an empty nonzero group reservation; native extended members remain open and must be closed individually. Group zero and individual channel IDs are ignored by `close_group`.

A group containing marked extended channels cannot also contain grouped standard channels, including group zero: their shared job would otherwise carry incompatible coinbase commitments. Opens or restorations that introduce this combination return `Error::IncompatibleChannelGroup`. Header-only standard channels are compatible because each receives its own standard job.

Run `cargo run --example channel_coinbase_outputs` for native fanout with local outputs and a check that share validation reconstructs exactly the coinbase sent to each channel.

## Validation and comparison

Run from this crate root:

```sh
cargo fmt --all -- --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo test --all-features
cargo run --example native_api
cargo run --example channel_coinbase_outputs
```

`prop_test` enables additional local template property tests. Local generators wrap native messages where orphan rules prevent upstream trait implementations. The retained suite covers mining rounds, restored prefixes, stale jobs, per-channel variants, extranonces, targets, and fanout. Integration tests add native consumer, wire, routing, group, budget, custom output, SegWit, and codec coverage.

```sh
cargo bench --bench migration
python3 scripts/compare-demand.py --iterations 10000 --output /tmp/comparison.txt
```

The comparison script archives the actual `demand/main` library source into a temporary directory, builds only a library consumer, and runs the same benchmark source against both revisions. It verifies identical encoded fixture and fanout bytes, measures allocations and time for equivalent borrowed/retained operations, and asserts zero allocations for the checked ID update. No application repository is needed. See [comparison notes](docs/performance.md) and [fixtures](fixtures/README.md).
