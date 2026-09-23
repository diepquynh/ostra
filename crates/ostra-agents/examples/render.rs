fn main() {
    let args: Vec<String> = std::env::args().collect();
    let agent: ostra_core::AgentName = args[1].parse().unwrap();
    let exec: ostra_core::ExecutorKind = args[2].parse().unwrap();
    print!("{}", ostra_agents::render_prompt(agent, exec).unwrap());
}
