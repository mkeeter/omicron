// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests for APIs against sled-based endpoints.

use dropshot::test_util::ClientTestContext;
use nexus_test_interface::NexusServer;
use nexus_test_utils::resource_helpers::create_physical_disk;
use nexus_test_utils::resource_helpers::delete_physical_disk;
use nexus_test_utils::resource_helpers::objects_list_page_authz;
use nexus_test_utils::start_sled_agent;
use nexus_test_utils::SLED_AGENT_UUID;
use nexus_test_utils_macros::nexus_test;
use omicron_nexus::external_api::views::{
    PhysicalDisk, PhysicalDiskType, Sled,
};
use omicron_nexus::internal_api::params as internal_params;
use omicron_sled_agent::sim;
use std::str::FromStr;
use uuid::Uuid;

type ControlPlaneTestContext =
    nexus_test_utils::ControlPlaneTestContext<omicron_nexus::Server>;

async fn switches_list(
    client: &ClientTestContext,
    switches_url: &str,
) -> Vec<Switch> {
    objects_list_page_authz::<Switch>(client, switches_url).await.items
}

#[nexus_test]
async fn test_switches_list(cptestctx: &ControlPlaneTestContext) {
    let client = &cptestctx.external_client;

    // Verify that there is one sled to begin with.
    let switches_url = "/v1/system/hardware/switches";
    assert_eq!(switches_list(&client, &switches_url).await.len(), 1);

    // Now start a few more sled agents.
    let nswitches = 3;
    let mut sas = Vec::with_capacity(nswitches);
    for _ in 0..nswitches {
        let sa_id = Uuid::new_v4();
        let log =
            cptestctx.logctx.log.new(o!( "sled_id" => sa_id.to_string() ));
        let addr = cptestctx.server.get_http_server_internal_address().await;
        let update_directory = std::path::Path::new("/should/not/be/used");
        sas.push(
            start_sled_agent(
                log,
                addr,
                sa_id,
                &update_directory,
                sim::SimMode::Explicit,
            )
            .await
            .unwrap(),
        );
    }

    // List switches again.
    let switches_found = switches_list(&client, &switches_url).await;
    assert_eq!(switches_found.len(), nswitches + 1);

    let sledids_found =
        switches_found.iter().map(|sv| sv.identity.id).collect::<Vec<Uuid>>();
    let mut sledids_found_sorted = sledids_found.clone();
    sledids_found_sorted.sort();
    assert_eq!(sledids_found, sledids_found_sorted);

    // Tear down the agents.
    for sa in sas {
        sa.http_server.close().await.unwrap();
    }
}

#[nexus_test]
async fn test_physical_disk_create_list_delete(
    cptestctx: &ControlPlaneTestContext,
) {
    let external_client = &cptestctx.external_client;
    let internal_client = &cptestctx.internal_client;

    // Verify that there is one sled to begin with.
    let switches_url = "/v1/system/hardware/switches";
    assert_eq!(switches_list(&external_client, &switches_url).await.len(), 1);

    // Verify that there are no disks.
    let disks_url =
        format!("/v1/system/hardware/switches/{SLED_AGENT_UUID}/disks");
    assert!(physical_disks_list(&external_client, &disks_url).await.is_empty());

    // Insert a new disk using the internal API, observe it in the external API
    let sled_id = Uuid::from_str(&SLED_AGENT_UUID).unwrap();
    create_physical_disk(
        &internal_client,
        "v",
        "s",
        "m",
        internal_params::PhysicalDiskKind::U2,
        sled_id,
    )
    .await;
    let disks = physical_disks_list(&external_client, &disks_url).await;
    assert_eq!(disks.len(), 1);
    assert_eq!(disks[0].vendor, "v");
    assert_eq!(disks[0].serial, "s");
    assert_eq!(disks[0].model, "m");
    assert_eq!(disks[0].disk_type, PhysicalDiskType::External);

    // Delete that disk using the internal API, observe it in the external API
    delete_physical_disk(&internal_client, "v", "s", "m", sled_id).await;
    assert!(physical_disks_list(&external_client, &disks_url).await.is_empty());
}
