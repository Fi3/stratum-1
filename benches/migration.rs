//! Identical operation/ownership workloads for native SRI and demand/main.
//! Run `python3 scripts/compare-demand.py` to compile this against both revisions.
use binary_sv2::GetSize;
use bitcoin::{
    consensus,
    hashes::{sha256, Hash},
    Amount, ScriptBuf, TxOut,
};
use roles_logic_sv2::{channel_logic::extended_to_standard_job, job_creator::JobsCreators};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};
#[cfg(demand_baseline)]
use {
    binary_sv2::Seq0255 as Path, mining_sv2::NewExtendedMiningJob as Job,
    template_distribution_sv2::NewTemplate as Template,
};
#[cfg(not(demand_baseline))]
use {
    binary_sv2::Seq0255Owned as Path, mining_sv2::NewExtendedMiningJobOwned as Job,
    template_distribution_sv2::NewTemplateOwned as Template,
};

struct CountAllocator;
static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
#[global_allocator]
static ALLOCATOR: CountAllocator = CountAllocator;
unsafe impl GlobalAlloc for CountAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ENABLED.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        // SAFETY: forward the caller's layout unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: allocations and layouts originate from the same system allocator.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ENABLED.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(size as u64, Ordering::Relaxed);
        }
        // SAFETY: preserve the caller's allocation contract and forward to System.
        unsafe { System.realloc(ptr, layout, size) }
    }
}
fn measure(name: &str, iterations: u64, mut operation: impl FnMut()) -> u64 {
    for _ in 0..100 {
        operation();
    }
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::Relaxed);
    let start = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    let duration = start.elapsed();
    ENABLED.store(false, Ordering::Relaxed);
    let allocations = ALLOCS.load(Ordering::Relaxed);
    println!(
        "{name},{:.1},{:.3},{:.1}",
        duration.as_nanos() as f64 / iterations as f64,
        allocations as f64 / iterations as f64,
        BYTES.load(Ordering::Relaxed) as f64 / iterations as f64
    );
    allocations
}
#[cfg(demand_baseline)]
fn fixture() -> Job<'static> {
    build_fixture()
}
#[cfg(not(demand_baseline))]
fn fixture() -> Job {
    build_fixture()
}
#[cfg(demand_baseline)]
type OwnedJob = Job<'static>;
#[cfg(not(demand_baseline))]
type OwnedJob = Job;
fn build_fixture() -> OwnedJob {
    let server_output = TxOut {
        value: Amount::ZERO,
        script_pubkey: ScriptBuf::from_bytes(
            [vec![0x6a, 0x24, 0xaa, 0x21, 0xa9, 0xed], vec![0; 32]].concat(),
        ),
    };
    let mut template = Template {
        template_id: 1000,
        future_template: true,
        version: 0x20000000,
        coinbase_tx_version: 2,
        coinbase_prefix: vec![3, 0x40, 0x0d, 0x03, 0].try_into().unwrap(),
        coinbase_tx_input_sequence: u32::MAX,
        coinbase_tx_value_remaining: 3_125_000_000,
        coinbase_tx_outputs_count: 1,
        coinbase_tx_outputs: consensus::serialize(&server_output).try_into().unwrap(),
        coinbase_tx_locktime: 0,
        merkle_path: Path::new((1..=12).map(|b| vec![b; 32].try_into().unwrap()).collect())
            .unwrap(),
    };
    let payout = TxOut {
        value: Amount::ZERO,
        script_pubkey: ScriptBuf::from_bytes([vec![0, 20], vec![1; 20]].concat()),
    };
    JobsCreators::new(16)
        .on_new_template(&mut template, true, vec![payout], 0)
        .unwrap()
}
fn main() {
    let iterations = std::env::var("ROLES_BENCH_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000);
    let job = fixture();
    let mut wire = binary_sv2::to_bytes(job.clone()).unwrap();
    let fanout = (0..64)
        .flat_map(|id| {
            binary_sv2::to_bytes(extended_to_standard_job(&job, &[7; 16], id, None, None).unwrap())
                .unwrap()
        })
        .collect::<Vec<_>>();
    println!(
        "fixture_payload_bytes={},fixture_sha256={},fanout_sha256={}",
        wire.len(),
        sha256::Hash::hash(&wire),
        sha256::Hash::hash(&fanout)
    );
    println!("operation,ns_per_op,allocations_per_op,allocated_bytes_per_op");
    assert_eq!(
        measure("get_size", iterations, || {
            black_box(job.get_size());
        }),
        0
    );
    measure("decode_borrowed", iterations, || {
        let decoded: mining_sv2::NewExtendedMiningJob =
            binary_sv2::from_bytes(black_box(&mut wire)).unwrap();
        black_box(decoded);
    });
    measure("decode_retained", iterations, || {
        let decoded: mining_sv2::NewExtendedMiningJob =
            binary_sv2::from_bytes(black_box(&mut wire)).unwrap();
        #[cfg(demand_baseline)]
        let retained = decoded.into_static();
        #[cfg(not(demand_baseline))]
        let retained = decoded.into_owned();
        black_box(retained);
    });
    measure("encode_owned_clone", iterations, || {
        black_box(binary_sv2::to_bytes(black_box(job.clone())).unwrap());
    });
    let mut request = [
        77u32.to_le_bytes().as_slice(),
        &[5],
        b"miner",
        &1f32.to_le_bytes(),
        &[255; 32],
    ]
    .concat();
    // Both paths include decoding the same request, update the decoded ID, and forward bytes.
    measure("decode_and_forward_id", iterations, || {
        request[..4].copy_from_slice(&77u32.to_le_bytes());
        let mut decoded: mining_sv2::OpenStandardMiningChannel =
            binary_sv2::from_bytes(black_box(&mut request)).unwrap();
        decoded.update_id(42);
        black_box(&decoded);
        #[cfg(not(demand_baseline))]
        roles_logic_sv2::frames::RequestIdUpdate {
            message_type: mining_sv2::MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
            original_id: 77,
            new_id: 42,
        }
        .apply_payload(
            mining_sv2::MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
            &mut request,
        )
        .unwrap();
        assert_eq!(&request[..4], &42u32.to_le_bytes());
        black_box(&request);
    });
    #[cfg(not(demand_baseline))]
    assert_eq!(
        measure("checked_id_patch_only", iterations, || {
            request[..4].copy_from_slice(&77u32.to_le_bytes());
            roles_logic_sv2::frames::RequestIdUpdate {
                message_type: mining_sv2::MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
                original_id: 77,
                new_id: 42,
            }
            .apply_payload(
                mining_sv2::MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
                black_box(&mut request),
            )
            .unwrap();
            black_box(&request);
        }),
        0
    );
    // Retain all 64 native standard jobs, matching the original fanout ownership contract.
    let extranonce = [7; 16];
    measure("fanout_64_retained", iterations.min(1000), || {
        let jobs = (0..64)
            .map(|id| {
                extended_to_standard_job(black_box(&job), &extranonce, id, None, None).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(jobs.len(), 64);
        black_box(jobs);
    });
}
