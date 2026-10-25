fn main() {
    println!("cargo:rerun-if-changed=../../infra/migration/versions");
}
