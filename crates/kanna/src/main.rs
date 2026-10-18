fn main() -> std::process::ExitCode {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    kanna::main()
}
