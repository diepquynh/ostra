fn main() -> anyhow::Result<()> {
    ostra_server::cli::main_with(ostra_sdk::Registry::new())
}
