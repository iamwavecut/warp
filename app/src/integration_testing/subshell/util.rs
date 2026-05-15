/// Produces a user/host pair for testing a given remote shell.
pub fn user_host(shell: &str) -> String {
    std::env::var("WARP_INTEGRATION_SSH_USER_HOST").unwrap_or_else(|_| format!("{shell}@localhost"))
}
/// Produces a user/host pair for remote-server tests.
pub fn remote_server_user_host(shell: &str) -> String {
    std::env::var("WARP_INTEGRATION_REMOTE_SERVER_USER_HOST")
        .unwrap_or_else(|_| format!("{shell}@localhost"))
}

/// Produces the full ssh command to run to ssh into a given remote shell.
pub fn ssh_command(shell: &str, should_use_ssh_wrapper: bool) -> String {
    let mut args = vec![
        if should_use_ssh_wrapper {
            "ssh".to_string()
        } else {
            "command ssh".to_string()
        },
        user_host(shell),
    ];
    let port = std::env::var("WARP_INTEGRATION_SSH_PORT").unwrap_or_else(|_| "22".to_string());
    let proxy_command = std::env::var("WARP_INTEGRATION_SSH_PROXY_COMMAND").ok();
    args.push("-p".to_string());
    args.push(port);
    let proxy_arg = proxy_command.map(|command| format!("-o ProxyCommand=\"{command}\""));
    if let Some(proxy_arg) = proxy_arg {
        args.push(proxy_arg);
    }
    args.extend([
        "-o StrictHostKeyChecking=no".to_string(),
        "-o UserKnownHostsFile=/dev/null".to_string(),
    ]);
    args.join(" ")
}

/// Produces the full ssh command to connect to the dedicated remote-server test host.
pub fn remote_server_ssh_command(shell: &str, should_use_ssh_wrapper: bool) -> String {
    let mut args = vec![
        if should_use_ssh_wrapper {
            "ssh".to_string()
        } else {
            "command ssh".to_string()
        },
        remote_server_user_host(shell),
    ];
    let port =
        std::env::var("WARP_INTEGRATION_REMOTE_SERVER_PORT").unwrap_or_else(|_| "22".to_string());
    let proxy_command = std::env::var("WARP_INTEGRATION_REMOTE_SERVER_PROXY_COMMAND").ok();
    args.push("-p".to_string());
    args.push(port);
    let proxy_arg = proxy_command.map(|command| format!("-o ProxyCommand=\"{command}\""));
    if let Some(proxy_arg) = proxy_arg {
        args.push(proxy_arg);
    }
    args.extend([
        "-o PreferredAuthentications=password".to_string(),
        "-o PubkeyAuthentication=no".to_string(),
        "-o StrictHostKeyChecking=no".to_string(),
        "-o UserKnownHostsFile=/dev/null".to_string(),
    ]);
    args.join(" ")
}
