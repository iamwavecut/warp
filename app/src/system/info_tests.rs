use super::SystemInfo;

#[test]
fn all_processes_refresh_does_not_sample_cpu_or_memory() {
    let all_processes = SystemInfo::all_processes_refresh_kind();
    assert!(!all_processes.cpu());
    assert!(!all_processes.memory());

    let current_process = SystemInfo::refresh_kind();
    assert!(current_process.cpu());
    assert!(current_process.memory());
}
