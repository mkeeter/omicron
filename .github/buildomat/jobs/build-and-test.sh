#!/bin/bash
#:
#: name = "build-and-test (helios)"
#: variety = "basic"
#: target = "helios-2.0"
#: rust_toolchain = "1.70.0"
#: output_rules = [
#:	"/var/tmp/omicron_tmp/*",
#:	"!/var/tmp/omicron_tmp/crdb-base*",
#:	"!/var/tmp/omicron_tmp/rustc*",
#: ]

set -o errexit
set -o pipefail
set -o xtrace

cargo --version
rustc --version

#
# Set up a custom temporary directory within whatever one we were given so that
# we can check later whether we left detritus around.
#
TEST_TMPDIR='/var/tmp/omicron_tmp'
echo "tests will store output in $TEST_TMPDIR" >&2
mkdir "$TEST_TMPDIR"

#
# Set up our PATH for the test suite.
#
source ./env.sh

banner prerequisites
ptime -m bash ./tools/install_builder_prerequisites.sh -y

#
# We build with:
#
# - RUSTFLAGS="-D warnings" RUSTDOCFLAGS="-D warnings": disallow warnings
#   in CI builds.  This can result in breakage when the toolchain is
#   updated, but that should only happen with a change to the repo, which
#   gives us an opportunity to find and fix any newly-introduced warnings.
#
# - `--locked`: do not update Cargo.lock when building.  Checking in
#   Cargo.lock ensures that everyone is using the same dependencies and
#   also gives us a record of which dependencies were used for each CI
#   run.  Building with `--locked` ensures that the checked-in Cargo.lock
#   is up to date.
#
# We don't use `--workspace` here because we're not prepared to run tests
# from end-to-end-tests.
#
# We apply our own timeout to ensure that we get a normal failure on timeout
# rather than a buildomat timeout.  See oxidecomputer/buildomat#8.
banner test
export RUSTFLAGS="-D warnings"
export RUSTDOCFLAGS="-D warnings"
export TMPDIR=$TEST_TMPDIR
export RUST_BACKTRACE=1
ptime -m timeout 2h cargo test --locked --verbose --no-fail-fast

#
# Make sure that we have left nothing around in $TEST_TMPDIR.  The easiest way
# to check is to try to remove it with `rmdir`.
#
unset TMPDIR
echo "files in $TEST_TMPDIR (none expected on success):" >&2
find "$TEST_TMPDIR" -ls
rmdir "$TEST_TMPDIR"
