//! Nexus startup task to load hardcoded data into the database
//!
//! Initial populating of the CockroachDB database happens in two different ways:
//!
//! 1. During "rack setup" (or during `omicron-dev db-run` or test suite
//!    initialization), we create the omicron database, schema, and the *bare
//!    minimum* data that needs to be there.
//! 2. Every time Nexus starts up, we attempts to insert a bunch of built-in
//!    users, roles, role assignments, silo, etc. into the database. We retry
//!    this until we successfully inserts everything we expect to or run into
//!    some unknown user input problem (e.g., an unexpected conflict, or a SQL
//!    syntax error, or something like that).
//!
//! This file implements this second process.
//!
//! As much as possible, data should be inserted using the second process.
//! That's because rack setup only ever happens once, so any data we add that
//! way will never get added to systems that were deployed on a previous version
//! of Omicron. On the other hand, if data is inserted at Nexus startup, then
//! the data will be automatically inserted on upgrade. That's good: that means
//! for the most part, if you want to add a new built-in user, you can just do
//! it and expect it to be there when your code is running.
//!
//! When Nexus starts up and goes to populate data, there are a few cases to
//! consider:
//!
//! * Nexus comes up and CockroachDB is not available. It should retry until
//!   CockroachDB is available.
//! * Nexus comes up and none of its data is present. It should go ahead and
//!   insert it.
//! * Nexus comes up and some of its data is present. It should still go ahead
//!   and insert it, ignoring primary key conflicts (on the assumption that it's
//!   the same data). This deals with crashes during a previous attempt, but
//!   also the upgrade case mentioned above: future versions of Nexus can
//!   deliver more data knowing it will be inserted the first time the new
//!   version of Nexus comes online.
//! * Nexus comes up and runs into a non-retryable problem doing any of this
//!   (e.g., SQL syntax error). It logs an error. This should eventually raise a
//!   support case. It shouldn't ever happen.
//!
//! To help do this correctly, we've defined the [`Populator`] trait.  This lets
//! you define a single data-insertion step.  We have tests that ensure that
//! each populator behaves as expected in the above ways.

use crate::context::OpContext;
use crate::db::{self, DataStore};
use crate::external_api::params;
use futures::future::BoxFuture;
use futures::FutureExt;
use lazy_static::lazy_static;
use omicron_common::api::external::Error;
use omicron_common::api::external::IdentityMetadataCreateParams;
use omicron_common::api::external::Name;
use omicron_common::backoff;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum PopulateStatus {
    NotDone,
    Done,
    Failed(String),
}

/// Auxiliary data necessary to populate the database.
pub struct PopulateArgs {
    rack_id: Uuid,
}

impl PopulateArgs {
    pub fn new(rack_id: Uuid) -> Self {
        Self { rack_id }
    }
}

pub fn populate_start(
    opctx: OpContext,
    datastore: Arc<DataStore>,
    args: PopulateArgs,
) -> tokio::sync::watch::Receiver<PopulateStatus> {
    let (tx, rx) = tokio::sync::watch::channel(PopulateStatus::NotDone);

    tokio::spawn(async move {
        let result = populate(&opctx, &datastore, &args).await;
        if let Err(error) = tx.send(match result {
            Ok(()) => PopulateStatus::Done,
            Err(message) => PopulateStatus::Failed(message),
        }) {
            error!(opctx.log, "nobody waiting for populate: {:#}", error)
        }
    });

    rx
}

async fn populate(
    opctx: &OpContext,
    datastore: &DataStore,
    args: &PopulateArgs,
) -> Result<(), String> {
    for p in *ALL_POPULATORS {
        let db_result = backoff::retry_notify(
            backoff::retry_policy_internal_service(),
            || async {
                p.populate(opctx, datastore, args).await.map_err(|error| {
                    match &error {
                        Error::ServiceUnavailable { .. } => {
                            backoff::BackoffError::transient(error)
                        }
                        _ => backoff::BackoffError::Permanent(error),
                    }
                })
            },
            |error, delay| {
                warn!(
                    opctx.log,
                    "failed to populate built-in {:?}; will retry in {:?}",
                    p,
                    delay;
                    "error_message" => ?error,
                );
            },
        )
        .await;

        if let Err(error) = &db_result {
            // TODO-autonomy this should raise an alert, bump a counter, or raise
            // some other red flag that something is wrong.  (This should be
            // unlikely in practice.)
            error!(opctx.log,
                "gave up trying to populate built-in {:?}", p;
                "error_message" => ?error
            );
        }

        db_result.map_err(|error| error.to_string())?;
    }

    Ok(())
}

/// Each Populator is a simple call into the datastore to load a chunk of
/// built-in (fixed) data that's shipped with Nexus
///
/// Each Populator should only do one thing.
trait Populator: std::fmt::Debug + Send + Sync {
    fn populate<'a, 'b>(
        &self,
        opctx: &'a OpContext,
        datastore: &'a DataStore,
        args: &'a PopulateArgs,
    ) -> BoxFuture<'b, Result<(), Error>>
    where
        'a: 'b;
}

/// Populates the built-in users
#[derive(Debug)]
struct PopulateBuiltinUsers;
impl Populator for PopulateBuiltinUsers {
    fn populate<'a, 'b>(
        &self,
        opctx: &'a OpContext,
        datastore: &'a DataStore,
        _args: &'a PopulateArgs,
    ) -> BoxFuture<'b, Result<(), Error>>
    where
        'a: 'b,
    {
        async { datastore.load_builtin_users(opctx).await.map(|_| ()) }.boxed()
    }
}

/// Populates the built-in roles
#[derive(Debug)]
struct PopulateBuiltinRoles;
impl Populator for PopulateBuiltinRoles {
    fn populate<'a, 'b>(
        &self,
        opctx: &'a OpContext,
        datastore: &'a DataStore,
        _args: &'a PopulateArgs,
    ) -> BoxFuture<'b, Result<(), Error>>
    where
        'a: 'b,
    {
        async { datastore.load_builtin_roles(opctx).await.map(|_| ()) }.boxed()
    }
}

/// Populates the built-in role assignments
#[derive(Debug)]
struct PopulateBuiltinRoleAssignments;
impl Populator for PopulateBuiltinRoleAssignments {
    fn populate<'a, 'b>(
        &self,
        opctx: &'a OpContext,
        datastore: &'a DataStore,
        _args: &'a PopulateArgs,
    ) -> BoxFuture<'b, Result<(), Error>>
    where
        'a: 'b,
    {
        async { datastore.load_builtin_role_asgns(opctx).await.map(|_| ()) }
            .boxed()
    }
}

/// Populates the built-in silo
#[derive(Debug)]
struct PopulateBuiltinSilos;
impl Populator for PopulateBuiltinSilos {
    fn populate<'a, 'b>(
        &self,
        opctx: &'a OpContext,
        datastore: &'a DataStore,
        _args: &'a PopulateArgs,
    ) -> BoxFuture<'b, Result<(), Error>>
    where
        'a: 'b,
    {
        async { datastore.load_builtin_silos(opctx).await.map(|_| ()) }.boxed()
    }
}

/// Populates the "test-privileged" and "test-unprivileged" silo users
// TODO-security Once we have a proper bootstrapping mechanism, we should not
// need to do this.  But right now, if you don't do this, then there will be no
// identities that you can use to do anything else -- including create other
// users or even set up a Silo that's connected to an identity provider.  This
// is needed for interactive use of Nexus (e.g., demos or just working on Nexus)
// as well as the test suite.
#[derive(Debug)]
struct PopulateSiloUsers;
impl Populator for PopulateSiloUsers {
    fn populate<'a, 'b>(
        &self,
        opctx: &'a OpContext,
        datastore: &'a DataStore,
        _args: &'a PopulateArgs,
    ) -> BoxFuture<'b, Result<(), Error>>
    where
        'a: 'b,
    {
        async { datastore.load_silo_users(opctx).await.map(|_| ()) }.boxed()
    }
}

/// Populates the role assignments for the "test-privileged" user
#[derive(Debug)]
struct PopulateSiloUserRoleAssignments;
impl Populator for PopulateSiloUserRoleAssignments {
    fn populate<'a, 'b>(
        &self,
        opctx: &'a OpContext,
        datastore: &'a DataStore,
        _args: &'a PopulateArgs,
    ) -> BoxFuture<'b, Result<(), Error>>
    where
        'a: 'b,
    {
        async {
            datastore.load_silo_user_role_assignments(opctx).await.map(|_| ())
        }
        .boxed()
    }
}

#[derive(Debug)]
struct PopulateFleet;
impl Populator for PopulateFleet {
    fn populate<'a, 'b>(
        &self,
        opctx: &'a OpContext,
        datastore: &'a DataStore,
        _args: &'a PopulateArgs,
    ) -> BoxFuture<'b, Result<(), Error>>
    where
        'a: 'b,
    {
        async {
            let id = *db::fixed_data::FLEET_ID;
            datastore
                .virtual_provisioning_collection_create(
                    opctx,
                    db::model::VirtualProvisioningCollection::new(
                        id,
                        db::model::CollectionTypeProvisioned::Fleet,
                    ),
                )
                .await?;

            Ok(())
        }
        .boxed()
    }
}

#[derive(Debug)]
struct PopulateRack;
impl Populator for PopulateRack {
    fn populate<'a, 'b>(
        &self,
        opctx: &'a OpContext,
        datastore: &'a DataStore,
        args: &'a PopulateArgs,
    ) -> BoxFuture<'b, Result<(), Error>>
    where
        'a: 'b,
    {
        async {
            datastore
                .rack_insert(opctx, &db::model::Rack::new(args.rack_id))
                .await?;

            let params = params::IpPoolCreate {
                identity: IdentityMetadataCreateParams {
                    name: "oxide-service-pool".parse::<Name>().unwrap(),
                    description: String::from("IP Pool for Oxide Services"),
                },
            };
            datastore
                .ip_pool_create(opctx, &params, /*internal=*/ true)
                .await
                .map(|_| ())
                .or_else(|e| match e {
                    Error::ObjectAlreadyExists { .. } => Ok(()),
                    _ => Err(e),
                })?;

            let params = params::IpPoolCreate {
                identity: IdentityMetadataCreateParams {
                    name: "default".parse::<Name>().unwrap(),
                    description: String::from("default IP pool"),
                },
            };
            datastore
                .ip_pool_create(opctx, &params, /*internal=*/ false)
                .await
                .map(|_| ())
                .or_else(|e| match e {
                    Error::ObjectAlreadyExists { .. } => Ok(()),
                    _ => Err(e),
                })?;

            Ok(())
        }
        .boxed()
    }
}

lazy_static! {
    static ref ALL_POPULATORS: [&'static dyn Populator; 8] = [
        &PopulateBuiltinUsers,
        &PopulateBuiltinRoles,
        &PopulateBuiltinRoleAssignments,
        &PopulateBuiltinSilos,
        &PopulateSiloUsers,
        &PopulateSiloUserRoleAssignments,
        &PopulateFleet,
        &PopulateRack,
    ];
}

#[cfg(test)]
mod test {
    use super::PopulateArgs;
    use super::Populator;
    use super::ALL_POPULATORS;
    use crate::authn;
    use crate::authz;
    use crate::context::OpContext;
    use crate::db;
    use anyhow::Context;
    use nexus_test_utils::db::test_setup_database;
    use omicron_common::api::external::Error;
    use omicron_test_utils::dev;
    use std::sync::Arc;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_populators() {
        for p in *ALL_POPULATORS {
            do_test_populator_idempotent(p).await;
        }
    }

    async fn do_test_populator_idempotent(p: &dyn Populator) {
        let logctx = dev::test_setup_log("test_populator");
        let mut db = test_setup_database(&logctx.log).await;
        let cfg = db::Config { url: db.pg_config().clone() };
        let pool = Arc::new(db::Pool::new(&cfg));
        let datastore = Arc::new(db::DataStore::new(pool));
        let opctx = OpContext::for_background(
            logctx.log.clone(),
            Arc::new(authz::Authz::new(&logctx.log)),
            authn::Context::internal_db_init(),
            Arc::clone(&datastore),
        );
        let log = &logctx.log;

        let args = PopulateArgs::new(Uuid::new_v4());

        // Running each populator once under normal conditions should work.
        info!(&log, "populator {:?}, run 1", p);
        p.populate(&opctx, &datastore, &args)
            .await
            .with_context(|| format!("populator {:?} (try 1)", p))
            .unwrap();

        // It should also work fine to run it again.
        info!(&log, "populator {:?}, run 2 (idempotency check)", p);
        p.populate(&opctx, &datastore, &args)
            .await
            .with_context(|| {
                format!(
                    "populator {:?}: expected to be idempotent, but it failed \
                     when run a second time",
                    p
                )
            })
            .unwrap();

        info!(&log, "cleaning up database");
        db.cleanup().await.unwrap();

        // Test again with the database offline.  In principle we could do this
        // immediately without creating a new pool and datastore.  However, the
        // pool's default behavior is to wait 30 seconds for a connection, which
        // makes this test take a long time.  (See the note in
        // nexus/src/db/pool.rs about this.)  So let's create a pool with an
        // arbitrarily short timeout now.  (We wouldn't want to do this above
        // because we do want to wait a bit when we expect things to work, in
        // case the test system is busy.)
        //
        // Anyway, if we try again with a broken database, we should get a
        // ServiceUnavailable error, which indicates a transient failure.
        let pool = Arc::new(db::Pool::new_failfast(&cfg));
        let datastore = Arc::new(db::DataStore::new(pool));
        let opctx = OpContext::for_background(
            logctx.log.clone(),
            Arc::new(authz::Authz::new(&logctx.log)),
            authn::Context::internal_db_init(),
            Arc::clone(&datastore),
        );

        info!(&log, "populator {:?}, with database offline", p);
        match p.populate(&opctx, &datastore, &args).await {
            Err(Error::ServiceUnavailable { .. }) => (),
            Ok(_) => panic!(
                "populator {:?}: unexpectedly succeeded with no database",
                p
            ),
            Err(error) => panic!(
                "populator {:?}: expected ServiceUnavailable when the database \
                was down, but got {:#} ({:?})",
                p,
                error,
                error
            ),
        };
        info!(&log, "populator {:?} done", p);
        logctx.cleanup_successful();
    }
}
