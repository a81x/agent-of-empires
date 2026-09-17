use clap::CommandFactory;
fn walk(cmd: &mut clap::Command, path: String, out: &mut String) {
    cmd.build();
    out.push_str(&format!("=== {path} hidden={}\n", cmd.is_hide_set()));
    out.push_str(&cmd.render_long_help().to_string());
    out.push_str(&cmd.render_help().to_string());
    for a in cmd.get_arguments() {
        out.push_str(&format!(
            "ARG {:?} hide={} help={:?} long={:?} vals={:?}\n",
            a.get_id(),
            a.is_hide_set(),
            a.get_help().map(|s| s.to_string()),
            a.get_long_help().map(|s| s.to_string()),
            a.get_possible_values()
                .iter()
                .map(|p| (p.get_name().to_string(), p.get_help().map(|h| h.to_string()), p.is_hide_set()))
                .collect::<Vec<_>>()
        ));
    }
    let names: Vec<String> = cmd.get_subcommands().map(|s| s.get_name().to_string()).collect();
    for n in names {
        let mut sub = cmd.find_subcommand_mut(&n).unwrap().clone();
        walk(&mut sub, format!("{path} {n}"), out);
    }
}
#[test]
fn dump_help() {
    let mut out = String::new();
    let mut cmd = agent_of_empires::cli::Cli::command();
    walk(&mut cmd, "aoe".into(), &mut out);
    std::fs::write(std::env::var("HELP_DUMP").unwrap(), out).unwrap();
}
