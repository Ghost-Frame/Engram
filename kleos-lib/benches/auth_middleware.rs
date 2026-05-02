//! Auth key-validation hot path.
//!
//! The kleos-server HTTP middleware delegates to `kleos_lib::auth::validate_key`
//! for hashing + DB lookup + scope resolution. That is the work that
//! runs on every authenticated request, so we bench it directly against
//! an in-memory tempfile DB. No axum, no network, no kleos-server
//! dependency (which would cause a workspace cycle).

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

mod common;

use kleos_lib::auth::{create_key, validate_key, Scope};
use kleos_lib::db::Database;

struct AuthFixture {
    db: Database,
    full_key: String,
}

fn setup(rt: &tokio::runtime::Runtime) -> AuthFixture {
    // Release-mode builds reject v1 (unpeppered) key issuance. Force a
    // deterministic pepper so the bench is hermetic + reproducible.
    if std::env::var_os("ENGRAM_API_KEY_PEPPER").is_none() {
        std::env::set_var("ENGRAM_API_KEY_PEPPER", "a".repeat(64));
    }
    rt.block_on(async {
        let db = Database::connect_memory().await.expect("connect_memory");
        let (_api_key, full_key) =
            create_key(&db, 1, "bench", vec![Scope::Read, Scope::Write], None)
                .await
                .expect("create_key");
        AuthFixture { db, full_key }
    })
}

fn bench_auth_valid_request(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let _guard = rt.enter();
    let fx = setup(&rt);

    let mut group = c.benchmark_group("auth_middleware/valid");
    group.throughput(Throughput::Elements(1));
    group.bench_function("bearer_ok", |b| {
        b.iter(|| {
            let ctx = rt
                .block_on(validate_key(&fx.db, &fx.full_key))
                .expect("validate_key");
            criterion::black_box(ctx);
        });
    });
    group.finish();
}

fn bench_auth_rejected_request(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let _guard = rt.enter();
    let fx = setup(&rt);
    let bad = "engram_00000000000000000000000000000000";

    let mut group = c.benchmark_group("auth_middleware/rejected");
    group.throughput(Throughput::Elements(1));
    group.bench_function("missing_bearer", |b| {
        b.iter(|| {
            let r = rt.block_on(validate_key(&fx.db, bad));
            criterion::black_box(r.is_err());
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_auth_valid_request,
    bench_auth_rejected_request
);
criterion_main!(benches);
