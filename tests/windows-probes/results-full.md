# Windows sandbox probe results

Run on: Microsoft Windows Server 2025 Datacenter Evaluation build 26100; host user probe-vm\ostraprobe

Status key: PASS = the design's assumption held; FAIL = a gap Phase 3 must handle; INCONCLUSIVE = undecidable on this VM; NEEDS_CLIENT = needs a Win11 client or the AV machine.

| verify | status | check | observed |
|---|---|---|---|
| 1 | PASS | spawn a program as another local user (CreateProcessWithLogonW) | spawned 6604 as OstraProbeA, exit 0; caller was elevated (see caveat) |
| 1 | PASS | unelevated caller (medium integrity) | caller integrity Medium; CreateProcessWithLogonW OK, child exit 0x0, child wrote: ran as probe-vm\ostraprobea pid 548 |
| 1 | PASS | confined process identity (CreateProcessWithLogonW target) | whoami = probe-vm\ostraprobea |
| 1 | PASS | privileges of the confined user | PRIVILEGES INFORMATION  \| ----------------------  \|   \| Privilege Name                Description                    State     \| ============================= ============================== ========  \| SeChangeNotifyPrivilege       Bypass traverse checking       Enabled   \| SeIncreaseWorkingSetPrivilege Increase a process working set Disabled |
| 2 | PASS | spawn child under a restricted primary token (no SeAssignPrimaryTokenPrivilege) | CreateProcessAsUserW returned success; child pid 9576 exit 0x0, wrote: ran as probe-vm\ostraprobea pid 9576 |
| 3 | PASS | Job Object kills the tree, including a nested-job grandchild | after kill: child alive=false, grandchild alive=false |
| 4 | PASS | list the host user's profile | C:\Users\ostraprobe: permission denied |
| 4 | PASS | list C:\Program Files | listed |
| 4 | PASS | list C:\Windows | listed |
| 4 | PASS | list C:\Windows\System32 | listed |
| 4 | INCONCLUSIVE | per-user toolchains (rustup/nvm/scoop/...) | none installed on this VM; re-run on a real developer profile |
| 5 | PASS | write inside the granted repo (positive control) | wrote allowed.txt |
| 5 | PASS | write .git\hooks via canonical | denied (PermissionDenied, os 5) |
| 5 | PASS | write .git\hooks via case-folded | denied (PermissionDenied, os 5) |
| 5 | PASS | write .git\hooks via trailing dot | denied (PermissionDenied, os 5) |
| 5 | PASS | write .git\hooks via trailing space | denied (PermissionDenied, os 5) |
| 5 | PASS | write .git\hooks via ADS on denied file | denied (PermissionDenied, os 5) |
| 5 | PASS | write .git\hooks via 8.3 short name | denied (PermissionDenied, os 5) |
| 5 | PASS | write protected .git\config | denied (PermissionDenied) |
| 5 | PASS | read protected .git\config (allowed) | read 26 bytes |
| 5 | PASS | read hidden secret via canonical | denied (PermissionDenied) |
| 5 | PASS | read hidden secret via case-folded | denied (PermissionDenied) |
| 5 | PASS | read hidden secret via ADS | no such stream (also not a read bypass) |
| 5 | PASS | rename .git (delete pin) | rename denied |
| 5 | PASS | rmdir /s .git (delete-child pin) | rmdir denied |
| 6 | PASS | hard link within the granted tree | link created, read through: Ok(9) |
| 6 | PASS | hard link to a hidden file then read | link created but read denied (no bypass) |
| 6 | PASS | hard link to a write-protected file | CreateHardLink denied (os 5) |
| 7 | PASS | connect to 127.0.0.1 under a block-all rule | connected: Windows Firewall does not filter loopback (a loopback-only server is also reachable by the sandbox user; restricting loopback to only the proxy port needs raw WFP) |
| 7 | PASS | connect to [::1] under a block-all rule | connected |
| 7 | INCONCLUSIVE | connect to the machine's own LAN IP under the block rule | connected to own IP <vm-lan-ip>: Windows treats the host's own addresses as local (like loopback), so a per-user Windows Firewall rule does not filter them; verifying the per-user block against a truly remote host needs a second machine and was not done here |
| 7 | FAIL | restrict loopback to only the proxy port | Windows Firewall cannot: loopback (and own-IP) traffic is exempt from its filtering, so limiting the sandbox user to only the proxy port on loopback needs a raw WFP filter, as the Phase 3 design already specifies |
| 7 | INCONCLUSIVE | DNS via Dnscache | documented gap: name resolution goes through the Dnscache service (a different account), so a per-user filter does not see it; not exercised live (VM has no resolver) |
| 8 | PASS | open the host process (different user) | OpenProcess failed, error 5 |
| 8 | FAIL | same-user sibling opens sibling (plain token) | OpenProcess succeeded (gap confirmed: siblings sharing a sandbox user are not isolated) |
| 8 | FAIL | per-execution restricting SID isolates same-user siblings | restricted opener still opened the sibling: restricting SIDs cannot isolate same-user processes, because a restricting set permissive enough to initialize includes a SID the sibling's process DACL also grants (account SID S-1-5-21-671473933-445965481-1138449831-1007). Isolation comes from one user per execution slot (pool design) and distinct accounts across slots |
| 8 | PASS | isolated restricting set (excludes account/logon SID) can run | child died at load (exit 0xc0000142, 0xc0000142 = DLL_INIT_FAILED): a restricting set that would isolate it cannot even start it |
| 9 | PASS | window station/desktop access for the sandbox user | granted BUILTIN\Users access to this session's window station and desktop; a real runner must provide a (private) desktop |
| 9 | NEEDS_CLIENT | desktop/input isolation (clipboard, SendInput, hooks) | needs an interactive Windows 11 desktop; this VM is headless over SSH. The station/desktop grant needed to start a process is recorded under verify 9 above |
| 10 | PASS | write own HKCU | exit Some(0): The operation completed successfully. |
| 10 | PASS | write HKLM Run key | exit Some(1): ERROR: Access is denied. |
| 11 | PASS | Task Scheduler task as the current user | schtasks (self): no process observed. A task or job owned by a sandbox user that is not interactively logged on (Ostra spawns it via CreateProcessWithLogonW, never a console logon) does not fire without stored credentials, so this mechanism does not escape here |
| 11 | PASS | Task Scheduler task as SYSTEM | no SYSTEM task exists after the attempt (refused without admin) |
| 11 | PASS | WMI Win32_Process.Create | WMI Win32_Process.Create: ran as the sandbox user (probe-vm\ostraprobea) via a service, i.e. outside the job. Contained by the ACL and network filter, but a job-kill would miss it, so Ostra's policy denylist must block the mechanism (as designed) |
| 11 | PASS | out-of-process COM (Shell.Application.ShellExecute) | COM Shell.Application: ran as the sandbox user (probe-vm\ostraprobea) via a service, i.e. outside the job. Contained by the ACL and network filter, but a job-kill would miss it, so Ostra's policy denylist must block the mechanism (as designed) |
| 11 | PASS | BITS SetNotifyCmdLine | the notify command did not fire within the timeout; like schtasks, a BITS job owned by a non-logged-on sandbox user does not run its notify command here, so it does not escape |
| 11 | INCONCLUSIVE | runas | runas prompts for a password interactively and cannot be scripted; not exercised |
| 12 | PASS | the sandbox user's own credential list | cmdkey /list: Currently stored credentials:    * NONE * |
| 12 | PASS | read the host user's profile (DPAPI/Credentials/browser stores) | read_dir on C:\Users\ostraprobe: permission denied |
| 13 | NEEDS_CLIENT | ConPTY fidelity per harness CLI | no harness CLIs (Claude, Codex, Grok, Agy) installed on this VM |
| 14 | NEEDS_CLIENT | Git Bash as the sandbox user | Git for Windows is not installed on this VM |
| 15 | NEEDS_CLIENT | mitigation policies vs node/cargo/rustc/python/java/harness | those toolchains are not installed on this VM; mitigation-policy compatibility must be measured where they are |
| 16 | PASS | recursive ACE cost over a synthetic tree | icacls /t granted a Modify ACE over 8000 files (40 dirs) in 1273 ms; a large real tree (node_modules/target) scales from here, favoring persistent per-workspace grants |
| 17 | PASS | Defender/SmartScreen reaction to the unsigned exe | the unsigned exe ran and created users, ACLs, and firewall rules without being blocked; RealTime=True AV=True; no winprobe detections; SmartScreen was not triggered (scp does not set the mark-of-the-web). Kaspersky's reaction still needs the user's own machine |

## Detail

- **verify 1 — spawn a program as another local user (CreateProcessWithLogonW)** [PASS]
  - expected: the call succeeds without special privilege (secondary logon service)
  - observed: spawned 6604 as OstraProbeA, exit 0; caller was elevated (see caveat)
- **verify 1 — unelevated caller (medium integrity)** [PASS]
  - expected: CreateProcessWithLogonW works from a medium-integrity (non-elevated) caller
  - observed: caller integrity Medium; CreateProcessWithLogonW OK, child exit 0x0, child wrote: ran as probe-vm\ostraprobea pid 548
- **verify 1 — confined process identity (CreateProcessWithLogonW target)** [PASS]
  - expected: runs as OstraProbeA, not the host user
  - observed: whoami = probe-vm\ostraprobea
- **verify 1 — privileges of the confined user** [PASS]
  - expected: no admin privileges (SeAssignPrimaryTokenPrivilege absent)
  - observed: PRIVILEGES INFORMATION  | ----------------------  |   | Privilege Name                Description                    State     | ============================= ============================== ========  | SeChangeNotifyPrivilege       Bypass traverse checking       Enabled   | SeIncreaseWorkingSetPrivilege Increase a process working set Disabled
- **verify 2 — spawn child under a restricted primary token (no SeAssignPrimaryTokenPrivilege)** [PASS]
  - expected: CreateProcessAsUserW succeeds with a restricted copy of the caller's own token
  - observed: CreateProcessAsUserW returned success; child pid 9576 exit 0x0, wrote: ran as probe-vm\ostraprobea pid 9576
- **verify 3 — Job Object kills the tree, including a nested-job grandchild** [PASS]
  - expected: child and nested-job grandchild both dead after TerminateJobObject
  - observed: after kill: child alive=false, grandchild alive=false
- **verify 4 — list the host user's profile** [PASS]
  - expected: denied
  - observed: C:\Users\ostraprobe: permission denied
- **verify 4 — list C:\Program Files** [PASS]
  - expected: system trees are readable
  - observed: listed
- **verify 4 — list C:\Windows** [PASS]
  - expected: system trees are readable
  - observed: listed
- **verify 4 — list C:\Windows\System32** [PASS]
  - expected: system trees are readable
  - observed: listed
- **verify 4 — per-user toolchains (rustup/nvm/scoop/...)** [INCONCLUSIVE]
  - expected: list which per-user toolchains a sandbox user cannot reach
  - observed: none installed on this VM; re-run on a real developer profile
- **verify 5 — write inside the granted repo (positive control)** [PASS]
  - expected: writing a granted file succeeds
  - observed: wrote allowed.txt
- **verify 5 — write .git\hooks via canonical** [PASS]
  - expected: denied
  - observed: denied (PermissionDenied, os 5)
- **verify 5 — write .git\hooks via case-folded** [PASS]
  - expected: denied
  - observed: denied (PermissionDenied, os 5)
- **verify 5 — write .git\hooks via trailing dot** [PASS]
  - expected: denied
  - observed: denied (PermissionDenied, os 5)
- **verify 5 — write .git\hooks via trailing space** [PASS]
  - expected: denied
  - observed: denied (PermissionDenied, os 5)
- **verify 5 — write .git\hooks via ADS on denied file** [PASS]
  - expected: denied
  - observed: denied (PermissionDenied, os 5)
- **verify 5 — write .git\hooks via 8.3 short name** [PASS]
  - expected: denied
  - observed: denied (PermissionDenied, os 5)
- **verify 5 — write protected .git\config** [PASS]
  - expected: denied
  - observed: denied (PermissionDenied)
- **verify 5 — read protected .git\config (allowed)** [PASS]
  - expected: reading a write-protected file still works
  - observed: read 26 bytes
- **verify 5 — read hidden secret via canonical** [PASS]
  - expected: denied
  - observed: denied (PermissionDenied)
- **verify 5 — read hidden secret via case-folded** [PASS]
  - expected: denied
  - observed: denied (PermissionDenied)
- **verify 5 — read hidden secret via ADS** [PASS]
  - expected: denied
  - observed: no such stream (also not a read bypass)
- **verify 5 — rename .git (delete pin)** [PASS]
  - expected: denied
  - observed: rename denied
- **verify 5 — rmdir /s .git (delete-child pin)** [PASS]
  - expected: denied
  - observed: rmdir denied
- **verify 6 — hard link within the granted tree** [PASS]
  - expected: a link to a granted file works
  - observed: link created, read through: Ok(9)
- **verify 6 — hard link to a hidden file then read** [PASS]
  - expected: the ACL travels with the file: read through the link is still denied
  - observed: link created but read denied (no bypass)
- **verify 6 — hard link to a write-protected file** [PASS]
  - expected: cannot create a hard link to a write-protected file (write-attr denied)
  - observed: CreateHardLink denied (os 5)
- **verify 7 — connect to 127.0.0.1 under a block-all rule** [PASS]
  - expected: loopback is exempt from Windows Firewall; the loopback egress proxy is reachable
  - observed: connected: Windows Firewall does not filter loopback (a loopback-only server is also reachable by the sandbox user; restricting loopback to only the proxy port needs raw WFP)
- **verify 7 — connect to [::1] under a block-all rule** [PASS]
  - expected: IPv6 loopback is exempt too
  - observed: connected
- **verify 7 — connect to the machine's own LAN IP under the block rule** [INCONCLUSIVE]
  - expected: a per-user outbound block filters non-loopback egress
  - observed: connected to own IP <vm-lan-ip>: Windows treats the host's own addresses as local (like loopback), so a per-user Windows Firewall rule does not filter them; verifying the per-user block against a truly remote host needs a second machine and was not done here
- **verify 7 — restrict loopback to only the proxy port** [FAIL]
  - expected: Windows Firewall can restrict loopback egress to the proxy port range
  - observed: Windows Firewall cannot: loopback (and own-IP) traffic is exempt from its filtering, so limiting the sandbox user to only the proxy port on loopback needs a raw WFP filter, as the Phase 3 design already specifies
- **verify 7 — DNS via Dnscache** [INCONCLUSIVE]
  - expected: DNS resolution is not filtered by a per-user WFP/firewall rule
  - observed: documented gap: name resolution goes through the Dnscache service (a different account), so a per-user filter does not see it; not exercised live (VM has no resolver)
- **verify 8 — open the host process (different user)** [PASS]
  - expected: OpenProcess on the host is denied (error 5)
  - observed: OpenProcess failed, error 5
- **verify 8 — same-user sibling opens sibling (plain token)** [FAIL]
  - expected: documented gap: same sandbox user's processes can open each other
  - observed: OpenProcess succeeded (gap confirmed: siblings sharing a sandbox user are not isolated)
- **verify 8 — per-execution restricting SID isolates same-user siblings** [FAIL]
  - expected: a restricting SID denies a same-user sibling open
  - observed: restricted opener still opened the sibling: restricting SIDs cannot isolate same-user processes, because a restricting set permissive enough to initialize includes a SID the sibling's process DACL also grants (account SID S-1-5-21-671473933-445965481-1138449831-1007). Isolation comes from one user per execution slot (pool design) and distinct accounts across slots
- **verify 8 — isolated restricting set (excludes account/logon SID) can run** [PASS]
  - expected: confirms the dichotomy: an isolating restricting set cannot initialize
  - observed: child died at load (exit 0xc0000142, 0xc0000142 = DLL_INIT_FAILED): a restricting set that would isolate it cannot even start it
- **verify 9 — window station/desktop access for the sandbox user** [PASS]
  - expected: the sandbox user needs a grant on a window station+desktop to start a process
  - observed: granted BUILTIN\Users access to this session's window station and desktop; a real runner must provide a (private) desktop
- **verify 9 — desktop/input isolation (clipboard, SendInput, hooks)** [NEEDS_CLIENT]
  - expected: runs on a Windows 11 client with real toolchains
  - observed: needs an interactive Windows 11 desktop; this VM is headless over SSH. The station/desktop grant needed to start a process is recorded under verify 9 above
- **verify 10 — write own HKCU** [PASS]
  - expected: the sandbox user can write its own HKCU (its own hive, expected)
  - observed: exit Some(0): The operation completed successfully.
- **verify 10 — write HKLM Run key** [PASS]
  - expected: HKLM is read-only for the sandbox user (persistence blocked)
  - observed: exit Some(1): ERROR: Access is denied.
- **verify 11 — Task Scheduler task as the current user** [PASS]
  - expected: no escape to a higher-privileged identity
  - observed: schtasks (self): no process observed. A task or job owned by a sandbox user that is not interactively logged on (Ostra spawns it via CreateProcessWithLogonW, never a console logon) does not fire without stored credentials, so this mechanism does not escape here
- **verify 11 — Task Scheduler task as SYSTEM** [PASS]
  - expected: creating a SYSTEM task is refused
  - observed: no SYSTEM task exists after the attempt (refused without admin)
- **verify 11 — WMI Win32_Process.Create** [PASS]
  - expected: no escape to a higher-privileged identity
  - observed: WMI Win32_Process.Create: ran as the sandbox user (probe-vm\ostraprobea) via a service, i.e. outside the job. Contained by the ACL and network filter, but a job-kill would miss it, so Ostra's policy denylist must block the mechanism (as designed)
- **verify 11 — out-of-process COM (Shell.Application.ShellExecute)** [PASS]
  - expected: no escape to a higher-privileged identity
  - observed: COM Shell.Application: ran as the sandbox user (probe-vm\ostraprobea) via a service, i.e. outside the job. Contained by the ACL and network filter, but a job-kill would miss it, so Ostra's policy denylist must block the mechanism (as designed)
- **verify 11 — BITS SetNotifyCmdLine** [PASS]
  - expected: no escape to a higher-privileged identity
  - observed: the notify command did not fire within the timeout; like schtasks, a BITS job owned by a non-logged-on sandbox user does not run its notify command here, so it does not escape
- **verify 11 — runas** [INCONCLUSIVE]
  - expected: no escape to a higher-privileged identity
  - observed: runas prompts for a password interactively and cannot be scripted; not exercised
- **verify 12 — the sandbox user's own credential list** [PASS]
  - expected: cmdkey lists only the sandbox user's own credentials, none of the host user's
  - observed: cmdkey /list: Currently stored credentials:    * NONE *
- **verify 12 — read the host user's profile (DPAPI/Credentials/browser stores)** [PASS]
  - expected: the host user's profile is unreadable by the sandbox user
  - observed: read_dir on C:\Users\ostraprobe: permission denied
- **verify 13 — ConPTY fidelity per harness CLI** [NEEDS_CLIENT]
  - expected: runs on a Windows 11 client with real toolchains
  - observed: no harness CLIs (Claude, Codex, Grok, Agy) installed on this VM
- **verify 14 — Git Bash as the sandbox user** [NEEDS_CLIENT]
  - expected: runs on a Windows 11 client with real toolchains
  - observed: Git for Windows is not installed on this VM
- **verify 15 — mitigation policies vs node/cargo/rustc/python/java/harness** [NEEDS_CLIENT]
  - expected: runs on a Windows 11 client with real toolchains
  - observed: those toolchains are not installed on this VM; mitigation-policy compatibility must be measured where they are
- **verify 16 — recursive ACE cost over a synthetic tree** [PASS]
  - expected: measure the time to apply an inheritable ACE recursively (per-execution vs persistent grants)
  - observed: icacls /t granted a Modify ACE over 8000 files (40 dirs) in 1273 ms; a large real tree (node_modules/target) scales from here, favoring persistent per-workspace grants
- **verify 17 — Defender/SmartScreen reaction to the unsigned exe** [PASS]
  - expected: record what Defender and SmartScreen do with the unsigned exe and the setup step
  - observed: the unsigned exe ran and created users, ACLs, and firewall rules without being blocked; RealTime=True AV=True; no winprobe detections; SmartScreen was not triggered (scp does not set the mark-of-the-web). Kaspersky's reaction still needs the user's own machine
