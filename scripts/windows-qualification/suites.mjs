/**
 * Independent Windows diagnostic cargo suites.
 * One failure must not hide the remaining suites.
 */

export const compileSuites = [
  {
    name: "cargo check -p platform-windows",
    args: ["check", "-p", "platform-windows"],
    section: "BUILD PREP",
    diagnostic: "COMPILATION",
  },
  {
    name: "cargo check -p platform-windows --features mutation",
    args: ["check", "-p", "platform-windows", "--features", "mutation"],
    section: "BUILD PREP",
    diagnostic: "COMPILATION",
  },
  {
    name: "cargo check -p application",
    args: ["check", "-p", "application"],
    section: "BUILD PREP",
    diagnostic: "COMPILATION",
  },
  {
    name: "cargo check -p operation-executor",
    args: ["check", "-p", "operation-executor"],
    section: "BUILD PREP",
    diagnostic: "COMPILATION",
  },
  {
    name: "cargo check -p desktop",
    args: ["check", "-p", "desktop"],
    section: "BUILD PREP",
    diagnostic: "COMPILATION",
    requiresSidecar: true,
  },
  {
    name: "application windows_read_only_qualification --no-run (LNK probe)",
    args: [
      "test",
      "-p",
      "application",
      "--test",
      "windows_read_only_qualification",
      "--no-run",
    ],
    section: "READ-ONLY",
    diagnostic: "LINKER",
  },
];

export const volumeSuites = [
  {
    name: "Windows volume/path diagnostics",
    args: [
      "test",
      "-p",
      "platform-windows",
      "--features",
      "mutation",
      "--test",
      "windows_volume_path_diagnostics",
      "--",
      "--nocapture",
    ],
    section: "NTFS",
    diagnostic: "VOLUME",
  },
  {
    name: "Windows path matrix (DOS/verbatim/unicode/long/case)",
    args: [
      "test",
      "-p",
      "platform-windows",
      "--test",
      "windows_path_matrix",
      "--",
      "--nocapture",
    ],
    section: "NTFS",
    diagnostic: "PATH IDENTITY",
  },
];

export const ntfsSuites = [
  ["standard move/rename/move+rename", "standard_move_rename_and_move_plus_rename"],
  ["case-only rename", "case_only_rename_requires_and_supports_safe_staging"],
  ["case-only Undo", "case_only_rename_undo_restores_original_leaf"],
  ["exact + case-insensitive collision / no overwrite", "exact_and_case_insensitive_destination_collisions"],
  ["source disappearance", "source_disappearance_before_mutation"],
  ["source drift", "source_content_drift_before_mutation"],
  ["read-only source", "read_only_source_is_refused"],
  ["sharing violation", "sharing_violation_is_retryable"],
  ["ACL denial", "sandbox_only_delete_deny_acl"],
  ["junction/reparse", "junction_leaf_ancestor_and_destination_escape"],
  ["symlink", "file_symlink_leaf_is_refused"],
  ["long path", "verbatim_long_paths_move"],
  ["restart reconciliation / rollback", "fresh_adapter_reconciles"],
].map(([name, filter]) => ({
  name,
  args: [
    "test",
    "-p",
    "platform-windows",
    "--features",
    "mutation",
    "--test",
    "ntfs_qualification",
    filter,
    "--",
    "--nocapture",
  ],
  section: "NTFS",
  diagnostic: diagnosticForNtfs(filter),
}));

function diagnosticForNtfs(filter) {
  if (filter.includes("sharing")) {
    return "LOCKS";
  }
  if (filter.includes("acl")) {
    return "ACL";
  }
  if (filter.includes("junction") || filter.includes("symlink")) {
    return "REPARSE";
  }
  if (filter.includes("fresh_adapter") || filter.includes("undo")) {
    return "ROLLBACK";
  }
  return "NTFS";
}

export const pathIdentitySuites = [
  ["Unicode / non-ASCII identity", "unicode_and_non_ascii_paths"],
  ["stable case-insensitive identity", "case_insensitive_volume_reports_stable"],
  ["reserved device names", "reserved_device_names_are_refused"],
  ["long Unicode identity", "long_unicode_component_names"],
].map(([name, filter]) => ({
  name,
  args: [
    "test",
    "-p",
    "platform-windows",
    "--features",
    "mutation",
    "--test",
    "windows_native_paths",
    filter,
    "--",
    "--nocapture",
  ],
  section: "NTFS",
  diagnostic: "PATH IDENTITY",
}));

export const monitoringSuites = [
  ["create", "windows_watcher_observes_create"],
  ["modify", "windows_watcher_observes_modify"],
  ["rename", "windows_watcher_observes_rename"],
  ["delete", "windows_watcher_observes_delete"],
  ["directory rename", "windows_watcher_observes_directory_rename"],
  ["Unicode", "windows_watcher_handles_unicode"],
  ["burst", "windows_watcher_survives_burst"],
  ["restart", "windows_watcher_survives_restart"],
].map(([name, filter]) => ({
  name: `watcher ${name}`,
  args: [
    "test",
    "-p",
    "platform",
    "--test",
    "windows_watcher_qualification",
    filter,
    "--",
    "--nocapture",
  ],
  section: "MONITORING",
  diagnostic: "MONITORING",
}));

export const executorSuites = [
  {
    name: "operation-executor protocol + native handler",
    args: ["test", "-p", "operation-executor", "--", "--nocapture"],
    section: "EXECUTOR",
    diagnostic: "EXECUTOR",
  },
  {
    name: "M8 no-overwrite / journal / sandbox execution",
    args: [
      "test",
      "-p",
      "application",
      "--test",
      "milestone8_safety_gated_execution",
      "destination_collision_and_source_drift",
      "--",
      "--nocapture",
    ],
    section: "EXECUTOR",
    diagnostic: "EXECUTOR",
  },
  {
    name: "crash before mutation",
    args: [
      "test",
      "-p",
      "application",
      "--test",
      "milestone8_safety_gated_execution",
      "crash_before_move_is_recovered",
      "--",
      "--nocapture",
    ],
    section: "EXECUTOR",
    diagnostic: "CRASH RECOVERY",
  },
  {
    name: "crash after mutation before ack",
    args: [
      "test",
      "-p",
      "application",
      "--test",
      "milestone8_safety_gated_execution",
      "crash_after_move_before_commit",
      "--",
      "--nocapture",
    ],
    section: "EXECUTOR",
    diagnostic: "CRASH RECOVERY",
  },
  {
    name: "M8 qualification rollback / recovery",
    args: [
      "test",
      "-p",
      "application",
      "--test",
      "milestone8_qualification",
      "--",
      "--nocapture",
    ],
    section: "ROLLBACK",
    diagnostic: "ROLLBACK",
  },
];

export const readOnlySuites = [
  {
    name: "persistence encrypted database open",
    args: ["test", "-p", "persistence", "--lib", "--", "--nocapture"],
    section: "READ-ONLY",
    diagnostic: "LINKER",
  },
  {
    name: "Windows read-only product flow",
    args: [
      "test",
      "-p",
      "application",
      "--test",
      "windows_read_only_qualification",
      "--",
      "--nocapture",
    ],
    section: "READ-ONLY",
    diagnostic: "LINKER",
  },
  {
    name: "Safe scanner (Windows)",
    args: [
      "test",
      "-p",
      "application",
      "--test",
      "safe_scanner",
      "--",
      "--nocapture",
    ],
    section: "READ-ONLY",
    diagnostic: "LINKER",
  },
];

export const semanticSuites = [
  {
    name: "Windows ORT / Granite / USearch runtime",
    args: [
      "test",
      "-p",
      "search",
      "--test",
      "windows_runtime_qualification",
      "--",
      "--nocapture",
    ],
    section: "SEMANTIC",
    diagnostic: "SEMANTIC",
  },
];

export const sandboxSuites = [
  {
    name: "Windows sandbox safety assertions",
    args: [
      "test",
      "-p",
      "platform",
      "--test",
      "windows_sandbox_safety",
      "--",
      "--nocapture",
    ],
    section: "SANDBOX SAFETY",
    diagnostic: "SANDBOX SAFETY",
  },
];
