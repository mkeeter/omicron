// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Copyright 2023 Oxide Computer Company

use schemars::JsonSchema;

use crate::{ExecutionId, StepSpec};

#[derive(JsonSchema)]
pub(crate) enum TestSpec {}

impl StepSpec for TestSpec {
    type Component = String;
    type StepId = usize;
    type StepMetadata = serde_json::Value;
    type ProgressMetadata = serde_json::Value;
    type CompletionMetadata = serde_json::Value;
    type SkippedMetadata = serde_json::Value;
    type Error = anyhow::Error;
}

pub(crate) static TEST_EXECUTION_UUID: &str =
    "2cc08a14-5e96-4917-bc70-e98293a3b703";

pub fn test_execution_id() -> ExecutionId {
    ExecutionId(TEST_EXECUTION_UUID.parse().expect("valid UUID"))
}
