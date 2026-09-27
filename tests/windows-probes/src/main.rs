//! Standalone probe for the Windows sandbox design (WINDOWS_HANDOVER.md, Phase 2 "verify first").
//! One exe, several roles by argv. Run only on a throwaway VM. No Ostra code.
//!
//! Roles:
//!   winprobe setup           create probe users, the ACL test tree, and per-user firewall filters
//!   winprobe run             orchestrate: spawn confined probes, run host-side checks, write results.md
//!   winprobe cleanup         remove everything setup created
//!   winprobe asuser <grp> <out>   confined role (spawned as a probe user): run one check group
//!   winprobe <helper> ...    small helpers the roles spawn (sleeper, openone, ...)

mod confined;
mod defs;
mod host;
mod model;
mod sys;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let role = args.get(1).map(String::as_str).unwrap_or("");
    let code = match role {
        "setup" => host::setup(),
        "run" => host::run(),
        "cleanup" => host::cleanup(),
        "asuser" => confined::asuser(&args),
        "markself" => confined::markself(&args),
        "medspawn" => confined::medspawn(&args),
        "sleeper" => confined::sleeper(&args),
        "openone" => confined::openone(&args),
        "spinjob" => confined::spinjob(&args),
        "netfilter" => confined::netfilter(&args),
        _ => {
            eprintln!("roles: setup | run | cleanup | asuser <grp> <out> | (helpers)");
            2
        }
    };
    std::process::exit(code);
}
