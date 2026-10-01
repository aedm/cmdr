//! What a copy, move, or delete on S3 will cost, at list prices, for the line
//! the dialogs show ("About $0.02 at AWS list prices").
//!
//! The dialog's own scan preview supplies the files (`cached_cost_facts`), the
//! volumes supply the providers, `plan.rs` turns them into billed work per
//! provider, and `cmdr_s3::cost` prices it against the table `price_source.rs`
//! keeps current. ❗ No request goes to S3 for an estimate.

mod plan;
mod price_source;

use std::path::Path;

use cmdr_s3::S3Volume;
use serde::{Deserialize, Serialize};

pub use plan::CostedOperation;
use plan::{Sides, plan};

use crate::file_system::volume::manager::get_volume_manager;
use crate::file_system::write_operations::cached_cost_facts;

/// What a dialog asks about, once its scan preview has settled.
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CostEstimateRequest {
    pub operation: CostedOperation,
    /// The settled preview whose files the operation will touch.
    pub preview_id: String,
    pub source_volume_id: String,
    /// `None` for a delete.
    pub destination_volume_id: Option<String>,
}

/// One provider's share of the cost.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CostEstimate {
    /// Unrounded; the dialog rounds it to the currency and hides a zero.
    pub amount: f64,
    /// ISO 4217 (`USD`, `EUR`), for the formatter.
    pub currency: String,
    /// The provider as its prices page names it (`AWS`, `Cloudflare R2`).
    pub provider_label: String,
}

/// The cost of `request` per S3 provider it touches. Empty when neither end is
/// an S3 place with list prices, or the preview isn't settled.
pub fn estimate(request: &CostEstimateRequest, data_dir: Option<&Path>) -> Vec<CostEstimate> {
    let manager = get_volume_manager();
    let source = manager.get(&request.source_volume_id);
    let destination = request.destination_volume_id.as_deref().and_then(|id| manager.get(id));
    let source_s3 = source.as_deref().and_then(|v| v.as_any().downcast_ref::<S3Volume>());
    let destination_s3 = destination
        .as_deref()
        .and_then(|v| v.as_any().downcast_ref::<S3Volume>());
    if source_s3.is_none() && destination_s3.is_none() {
        return Vec::new();
    }
    let Some(facts) = cached_cost_facts(&request.preview_id) else {
        return Vec::new();
    };
    let sides = Sides {
        source: source_s3.map(S3Volume::cost_workload),
        destination: destination_s3.map(S3Volume::cost_workload),
        server_copy: matches!((source_s3, destination_s3), (Some(from), Some(to)) if to.copies_on_server_from(from)),
    };
    let table = price_source::current(data_dir);
    plan(request.operation, sides, &facts)
        .iter()
        .filter_map(|work| table.estimate(work))
        .map(|estimate| {
            log::debug!(target: "s3_costs", "{:?} at {}: {:?}", request.operation, estimate.provider_label, estimate.line_items);
            CostEstimate {
                amount: estimate.total,
                currency: estimate.currency,
                provider_label: estimate.provider_label,
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod plan_tests;
