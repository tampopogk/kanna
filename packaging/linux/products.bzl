"""Linux products are siblings of the Darwin bundle graph, with explicit targets."""

load("//tools/bazel:defs.bzl", "target_platform_transition")

_EXECUTABLES = {
    "desktop": "kanna-desktop",
    "worker": "kanna-worker",
    "daemon": "kanna-daemon",
    "cli": "kanna-cli",
    "mcp": "kanna-mcp",
    "server": "kanna-server",
    "transfer": "kanna-task-transfer",
    "recovery": "kanna-terminal-recovery",
}

def _linux_products_impl(ctx):
    outputs = []
    for attr, name in _EXECUTABLES.items():
        target = getattr(ctx.attr, attr)[0]
        files = target[DefaultInfo].files.to_list()
        if len(files) != 1:
            fail("%s must produce one executable" % attr)
        output = ctx.actions.declare_file(ctx.label.name + "/" + name)
        ctx.actions.run_shell(
            inputs = files,
            outputs = [output],
            arguments = [files[0].path, output.path],
            command = 'cp "$1" "$2" && chmod 755 "$2"',
            mnemonic = "LinuxProduct",
        )
        outputs.append(output)
    return [DefaultInfo(files = depset(outputs))]

linux_products = rule(
    implementation = _linux_products_impl,
    attrs = dict({
        name: attr.label(mandatory = True, cfg = target_platform_transition)
        for name in _EXECUTABLES
    }, **{
        "platform": attr.label(mandatory = True),
        "_allowlist_function_transition": attr.label(default = "@bazel_tools//tools/allowlists/function_transition_allowlist"),
    }),
)

def linux_product_targets():
    for arch in ("arm64", "x86_64"):
        for channel in ("production", "staging"):
            linux_products(
                name = "products_{}_{}".format(channel, arch),
                platform = "//tools/bazel:linux_" + arch,
                desktop = "//apps/desktop/src-tauri:kanna_desktop_{}bazel".format("staging_" if channel == "staging" else ""),
                worker = "//crates/kanna-worker:kanna_worker",
                daemon = "//crates/daemon:kanna_daemon",
                cli = "//crates/kanna-cli:kanna_cli",
                mcp = "//crates/kanna-mcp:kanna_mcp",
                server = "//crates/kanna-server:kanna_server",
                transfer = "//crates/task-transfer:kanna_task_transfer",
                recovery = "//packages/terminal-recovery:kanna_terminal_recovery",
            )

def _linux_deb_impl(ctx):
    if ctx.var["COMPILATION_MODE"] != "opt":
        fail("Linux product packages require -c opt")
    output = ctx.actions.declare_file(ctx.label.name + ".deb")
    report = ctx.actions.declare_file(ctx.label.name + ".json")
    manifest = ctx.actions.declare_file(ctx.label.name + ".inputs.json")
    products = {f.basename: f.path for f in ctx.files.products}
    if sorted(products) != sorted(_EXECUTABLES.values()):
        fail("Linux package requires exactly the eight declared product executables")
    ctx.actions.write(manifest, json.encode({
        "architecture": ctx.attr.architecture,
        "channel": ctx.attr.channel,
        "iteration": ctx.attr._iteration[LinuxIterationInfo].value,
        "products": products,
        "versionFile": ctx.file.version.path,
        "resources": [f.path for f in ctx.files.resources],
        "icons": [f.path for f in ctx.files.icons],
        "policy": ctx.file.policy.path,
        "tool": ctx.executable._artifact_tool.path,
        "output": output.path,
        "report": report.path,
    }))
    ctx.actions.run(
        executable = ctx.executable._package_tool,
        arguments = [ctx.file._entry.path, manifest.path],
        inputs = depset([manifest, ctx.file._entry, ctx.file.version, ctx.file.policy] + ctx.files.products + ctx.files.resources + ctx.files.icons + ctx.files._sources),
        tools = [ctx.attr._artifact_tool[DefaultInfo].files_to_run, ctx.attr._package_tool[DefaultInfo].files_to_run],
        outputs = [output, report],
        env = {"BAZEL_BINDIR": ctx.bin_dir.path, "JS_BINARY__NO_CD_BINDIR": "1"},
        mnemonic = "LinuxDeb",
        progress_message = "Auditing and assembling %s" % ctx.label.name,
    )
    return [DefaultInfo(files = depset([output, report]))]

linux_deb = rule(
    implementation = _linux_deb_impl,
    attrs = {
        "architecture": attr.string(mandatory = True, values = ["arm64", "x86_64"]),
        "channel": attr.string(mandatory = True, values = ["production", "staging"]),
        "_iteration": attr.label(default = "//packaging/linux:staging_iteration"),
        "products": attr.label(mandatory = True),
        "version": attr.label(default = "//:VERSION", allow_single_file = True),
        "resources": attr.label(default = "//:kanna_builtin_resources"),
        "icons": attr.label(default = "//apps/desktop/src-tauri:icons"),
        "policy": attr.label(default = "//packaging/linux:runtime-policy.json", allow_single_file = True),
        "_sources": attr.label(default = "//tools/kd:linux_package_sources"),
        "_entry": attr.label(default = "//tools/kd:src/runtime/linux-bazel-package.ts", allow_single_file = True),
        "_package_tool": attr.label(default = "//tools/kd:linux_package_tool", cfg = "exec", executable = True),
        "_artifact_tool": attr.label(default = "//packaging/linux:artifact_tool", cfg = "exec", executable = True),
    },
)

def linux_package_targets():
    for arch in ("arm64", "x86_64"):
        for channel in ("production", "staging"):
            linux_deb(
                name = "deb_{}_{}".format(channel, arch),
                architecture = arch,
                channel = channel,
                products = ":products_{}_{}".format(channel, arch),
            )

LinuxIterationInfo = provider(fields = ["value"])

def _iteration_impl(ctx):
    if ctx.build_setting_value < 1:
        fail("staging iteration must be positive")
    return [LinuxIterationInfo(value = ctx.build_setting_value)]

linux_iteration = rule(implementation = _iteration_impl, build_setting = config.int(flag = True))
