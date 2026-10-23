Reusable mining fixtures
=======================

`segwit-transaction.hex` is the real witness transaction in rust-bitcoin’s `segwit_transaction` test (`bitcoin` 0.32.102, MIT/CC0). Its txid is `f5864806e3565c34d1b41e716f72609d00b55ea5eac5b924c9719a842ef42206` and wtxid is `80b7d8a82d5d5bf92905b06f2014dd699e03837ca172e3a59d51426ebbe3e7f5`.

`reg-test-block.toml` at the crate root retains the demand mining-round fixture. The performance harness uses deterministic coinbase bytes and twelve Merkle siblings; both revisions use identical bytes and ownership requirements.
