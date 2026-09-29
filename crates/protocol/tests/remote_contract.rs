#[path = "support/remote_contract.rs"]
mod remote_contract;

#[test]
fn remote_typescript_fixture_matches_rust_serialization() {
    assert_eq!(
        remote_contract::typescript(),
        include_str!("../../../apps/remote-web/src/remote-contract.fixture.ts"),
        "Regenerate with cargo run -p nexus-protocol --example remote_contract, then run the remote client typecheck"
    );
}
