#[path = "../tests/support/remote_contract.rs"]
mod remote_contract;

fn main() {
    print!("{}", remote_contract::typescript());
}
