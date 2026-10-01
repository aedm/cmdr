//! The planner against hand-computed amounts at the bundled table's list prices
//! (`crates/cmdr-s3/src/cost/s3-prices.json`): AWS "PUT, COPY, POST, LIST"
//! $5.00/M, "GET and all other requests" $0.40/M, egress $0.09/GB; R2 Class A
//! $4.50/M, Class B $0.36/M; Wasabi storage $0.00780273/GB-month, 90 days.

use cmdr_s3::S3Provider;
use cmdr_s3::cost::{PriceTable, Workload};

use super::plan::{CostedOperation, Sides, plan};
use crate::file_system::volume::ScannedFile;
use crate::file_system::write_operations::ScanCostFacts;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
const DAY: u64 = 86_400;
const NOW: u64 = 1_790_000_000;

fn aws() -> Workload {
    Workload::for_provider_at(
        &S3Provider::Aws {
            region: "us-east-1".into(),
        },
        NOW,
    )
}
fn r2() -> Workload {
    Workload::for_provider_at(
        &S3Provider::R2 {
            account_id: "0123456789abcdef0123456789abcdef".into(),
        },
        NOW,
    )
}
fn wasabi() -> Workload {
    Workload::for_provider_at(
        &S3Provider::Wasabi {
            region: "eu-central-1".into(),
        },
        NOW,
    )
}

fn facts(files: &[(u64, Option<u64>)], dirs: usize) -> ScanCostFacts {
    ScanCostFacts {
        files: files.len(),
        dirs,
        bytes: files.iter().map(|(size, _)| size).sum(),
        per_file: Some(
            files
                .iter()
                .map(|&(size, modified_at)| ScannedFile { size, modified_at })
                .collect(),
        ),
    }
}

fn totals(workloads: &[Workload]) -> Vec<f64> {
    let table = PriceTable::bundled();
    workloads
        .iter()
        .map(|work| table.estimate(work).expect("a priced provider").total)
        .collect()
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "expected {expected}, got {actual}");
}

#[test]
fn nothing_on_s3_plans_nothing() {
    let sides = Sides {
        source: None,
        destination: None,
        server_copy: false,
    };
    assert!(plan(CostedOperation::Copy, sides, &facts(&[(MIB, None)], 0)).is_empty());
}

#[test]
fn an_upload_to_aws_is_a_put_and_a_head_per_file_and_folder() {
    let sides = Sides {
        source: None,
        destination: Some(aws()),
        server_copy: false,
    };
    let files: Vec<_> = (0..1_000).map(|_| (MIB, None)).collect();
    let planned = plan(CostedOperation::Copy, sides, &facts(&files, 2));
    // 1,000 files and two folder markers: 1,002 PUTs ($0.00501) and 1,002
    // verifying HEADs ($0.0004008).
    close(totals(&planned)[0], 0.0054108);
}

#[test]
fn a_download_from_aws_bills_its_bytes() {
    let sides = Sides {
        source: Some(aws()),
        destination: None,
        server_copy: false,
    };
    let planned = plan(CostedOperation::Copy, sides, &facts(&[(10 * GIB, None)], 0));
    // One GET and 10 GB × $0.09.
    close(totals(&planned)[0], 0.9000004);
}

#[test]
fn a_move_within_one_aws_account_copies_on_the_server_and_deletes_the_source() {
    let sides = Sides {
        source: Some(aws()),
        destination: Some(aws()),
        server_copy: true,
    };
    let planned = plan(CostedOperation::Move, sides, &facts(&[(MIB, None)], 1));
    // One workload: the account is billed once. The file: `CopyObject` and two
    // HEADs; the folder: its marker written at the destination (PUT + HEAD), and
    // at the source a capped listing plus the marker's free delete. The file's
    // delete is one free `DeleteObjects`.
    // PUT-class: COPY + marker PUT + LIST = 3 × $5/M; HEADs: 3 × $0.40/M.
    assert_eq!(planned.len(), 1);
    close(totals(&planned)[0], 3.0 * 5e-6 + 3.0 * 4e-7);
}

#[test]
fn a_copy_between_two_accounts_downloads_from_one_and_uploads_to_the_other() {
    let sides = Sides {
        source: Some(r2()),
        destination: Some(aws()),
        server_copy: false,
    };
    let planned = plan(CostedOperation::Copy, sides, &facts(&[(MIB, None), (MIB, None)], 0));
    let amounts = totals(&planned);
    // R2: two GETs at $0.36/M, egress free. AWS: two PUTs and two HEADs.
    close(amounts[0], 2.0 * 0.36e-6);
    close(amounts[1], 2.0 * 5e-6 + 2.0 * 4e-7);
}

#[test]
fn a_wasabi_delete_bills_the_young_objects_remaining_days() {
    let sides = Sides {
        source: Some(wasabi()),
        destination: None,
        server_copy: false,
    };
    let planned = plan(
        CostedOperation::Delete,
        sides,
        &facts(&[(GIB, Some(NOW - 30 * DAY)), (GIB, Some(NOW - 200 * DAY))], 1),
    );
    // Only the 30-day-old object: 1 GB × 60 days × $0.00780273 / 30.
    close(totals(&planned)[0], 60.0 * 0.00780273 / 30.0);
}

#[test]
fn a_move_off_wasabi_bills_early_deletion_on_the_source() {
    let sides = Sides {
        source: Some(wasabi()),
        destination: None,
        server_copy: false,
    };
    let planned = plan(CostedOperation::Move, sides, &facts(&[(GIB, Some(NOW))], 0));
    close(totals(&planned)[0], 90.0 * 0.00780273 / 30.0);
}

#[test]
fn a_copy_off_wasabi_deletes_nothing_and_costs_nothing() {
    let sides = Sides {
        source: Some(wasabi()),
        destination: None,
        server_copy: false,
    };
    let planned = plan(CostedOperation::Copy, sides, &facts(&[(GIB, Some(NOW))], 0));
    close(totals(&planned)[0], 0.0);
}

#[test]
fn without_a_per_file_list_the_bytes_spread_evenly_over_the_files() {
    let sides = Sides {
        source: None,
        destination: Some(aws()),
        server_copy: false,
    };
    let bare = ScanCostFacts {
        files: 4,
        dirs: 0,
        bytes: 400 * MIB,
        per_file: None,
    };
    let planned = plan(CostedOperation::Copy, sides, &bare);
    // Four 100 MiB files, each two 64 MiB-floor parts plus Create and Complete:
    // 16 PUT-class requests, and four HEADs.
    close(totals(&planned)[0], 16.0 * 5e-6 + 4.0 * 4e-7);
}
