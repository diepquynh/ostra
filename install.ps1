# Build Ostra, install ostra.exe, and run it as a per-user Task Scheduler task on Windows. The task starts at
# logon, needs no administrator rights, and starts the server again within 5 minutes when it stops.
#
# Usage:
#   .\install.ps1 [install] [ostra flags...]   build, install, and (re)start; for example --port 8080
#                                              The service listens on 127.0.0.1 unless a --bind flag says
#                                              otherwise; --bind 0.0.0.0 serves plain HTTP to the network
#   .\install.ps1 restart                      restart the service with the installed binary
#   .\install.ps1 status                       show whether the service runs
#   .\install.ps1 url                          print a fresh sign-in URL
#   .\install.ps1 uninstall                    stop and remove the service and the installed files
# Environment:
#   OSTRA_PREFIX     install prefix (default ~\.local; the binary goes to $OSTRA_PREFIX\bin)
#   SKIP_BUILD=1     install the existing target\release\ostra.exe without building
#   OSTRA_CONFIG, OSTRA_DATA_DIR   honored when saved as user environment variables, because the task starts
#                                  with the saved environment and not this shell's
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$scriptArgs = @($args)
$repo = $PSScriptRoot
$prefix = if ($env:OSTRA_PREFIX) { $env:OSTRA_PREFIX } else { Join-Path $HOME '.local' }
$binDir = Join-Path $prefix 'bin'
$bin = Join-Path $binDir 'ostra.exe'
$task = 'dev.ostra.server'
$conhost = Join-Path $env:SystemRoot 'System32\conhost.exe'
$dataDir = if ($env:OSTRA_DATA_DIR) { $env:OSTRA_DATA_DIR } else {
    Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'ostra'
}
$serverFile = Join-Path $dataDir 'server.json'

function Write-Step([string]$text) { Write-Host "==> $text" }

function Invoke-Checked([string]$what, [scriptblock]$command) {
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$what failed with exit code $LASTEXITCODE." }
}

function Get-SavedEnv([string]$name) {
    $value = [Environment]::GetEnvironmentVariable($name, 'User')
    if (-not $value) { $value = [Environment]::GetEnvironmentVariable($name, 'Machine') }
    $value
}

# The task runs ostra.exe with the user's saved environment, so a value set only in this shell would reach the
# installer but not the server.
function Assert-Environment {
    if ($env:OSTRA_ENV_FILE) {
        throw 'Unset OSTRA_ENV_FILE and save provider keys in the setup screen or as user environment variables, because the Windows task runs ostra.exe directly and applies no credentials file.'
    }
    foreach ($name in 'OSTRA_CONFIG', 'OSTRA_DATA_DIR') {
        $current = [Environment]::GetEnvironmentVariable($name, 'Process')
        if ($current -and $current -ne (Get-SavedEnv $name)) {
            throw "Save $name as a user environment variable (setx $name `"$current`") or unset it, then open a new terminal and run install.ps1 again, because the task starts with the saved environment and not this shell's."
        }
    }
}

# A terminal opened before rustup or Node.js was installed keeps its old PATH, so the saved entries a new
# terminal would see are added.
function Update-Path {
    $known = @($env:Path -split ';')
    foreach ($scope in 'User', 'Machine') {
        foreach ($dir in @([Environment]::GetEnvironmentVariable('Path', $scope) -split ';')) {
            if ($dir -and $known -notcontains $dir) {
                $env:Path = "$env:Path;$dir"
                $known += $dir
            }
        }
    }
}

function Assert-Tool([string]$name, [string]$install) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        throw "Install $install, then open a new terminal and run install.ps1 again, because the build needs $name and it is not on PATH."
    }
}

function Build-Ostra {
    Update-Path
    Assert-Tool 'npm' 'Node.js 24 (https://nodejs.org)'
    Assert-Tool 'cargo' 'Rust with rustup (https://rustup.rs)'
    Push-Location $repo
    try {
        # The binary embeds web\dist at compile time, so the UI is built first.
        if (-not (Test-Path node_modules)) {
            Write-Step 'Installing web dependencies'
            Invoke-Checked 'npm ci' { npm ci --no-audit --no-fund }
        }
        $stamp = 'web\dist\index.html'
        $stale = -not (Test-Path $stamp)
        if (-not $stale) {
            $built = (Get-Item $stamp).LastWriteTimeUtc
            $inputs = 'web\src', 'web\public', 'web\index.html', 'web\package.json', 'design\src', 'design\package.json'
            $stale = [bool](Get-ChildItem -Path $inputs -Recurse -File -ErrorAction SilentlyContinue |
                    Where-Object { $_.LastWriteTimeUtc -gt $built } | Select-Object -First 1)
        }
        if ($stale) {
            Write-Step 'Building the web UI'
            Invoke-Checked 'The web UI build' { npm run -s build --workspace web }
        } else {
            Write-Step 'Web UI is up to date'
        }
        Write-Step 'Building ostra (release)'
        Invoke-Checked 'cargo build' { cargo build --release -p ostra-server }
    } finally {
        Pop-Location
    }
}

function Get-RecordedServer {
    if (-not (Test-Path $serverFile)) { return $null }
    try { $id = (Get-Content -Raw $serverFile | ConvertFrom-Json).pid } catch { return $null }
    if (-not $id) { return $null }
    Get-Process -Id $id -ErrorAction SilentlyContinue
}

function Test-Installed($process) {
    $process.Path -eq $bin -or $process.Path -eq "$bin.old"
}

function Assert-NoOtherServer {
    $other = Get-RecordedServer
    if ($other) {
        throw "Stop the Ostra server running as process $($other.Id) ($($other.Path)), then run install.ps1 again, because two servers must not share the data folder $dataDir."
    }
}

function Get-OstraTask { Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue }

# Ending conhost leaves ostra.exe running, so the server is stopped first and conhost exits with it.
function Stop-OstraService {
    $server = Get-RecordedServer
    if ($server -and (Test-Installed $server)) {
        Stop-Process -Id $server.Id -Force
        $server.WaitForExit(10000) | Out-Null
    }
    if (Get-OstraTask) {
        Stop-ScheduledTask -TaskName $task
        # IgnoreNew drops a start while the task still counts as running.
        for ($i = 0; $i -lt 20 -and (Get-OstraTask).State -eq 'Running'; $i++) { Start-Sleep -Milliseconds 250 }
    }
}

# Windows command-line quoting, the rules CommandLineToArgvW reads back.
function ConvertTo-Argument([string]$value) {
    if ($value -ne '' -and $value -notmatch '[\s"]') { return $value }
    '"' + (($value -replace '(\\*)"', '$1$1\"') -replace '(\\+)$', '$1$1') + '"'
}

function Get-BindAddress([string[]]$flags) {
    for ($i = 0; $i -lt $flags.Count; $i++) {
        if ($flags[$i] -eq '--bind' -and $i + 1 -lt $flags.Count) { return $flags[$i + 1] }
        if ($flags[$i] -like '--bind=*') { return $flags[$i].Substring(7) }
    }
    $null
}

function Register-OstraTask([string[]]$flags) {
    $serve = @('serve', '--no-open')
    if (-not (Get-BindAddress $flags)) { $serve += '--bind', '127.0.0.1' }
    $serve += $flags
    # conhost --headless gives ostra.exe a console with no window, so nothing opens at logon.
    $line = (@('--headless', $bin) + $serve | ForEach-Object { ConvertTo-Argument $_ }) -join ' '
    $user = "$env:USERDOMAIN\$env:USERNAME"
    $action = New-ScheduledTaskAction -Execute $conhost -Argument $line -WorkingDirectory $HOME
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
    # conhost exits 0 however the server ends, so Task Scheduler never sees a failure to restart on. The
    # repetition starts a server that exited; IgnoreNew skips it while one runs.
    $trigger.Repetition = (New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes 5)).Repetition
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew `
        -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
    Register-ScheduledTask -TaskName $task -Action $action -Trigger $trigger -Settings $settings `
        -Principal $principal -Description 'Ostra engineering pipeline server' -Force | Out-Null
}

function Start-OstraService {
    $since = [DateTime]::UtcNow
    Start-ScheduledTask -TaskName $task
    # server.json is rewritten at each start, so a copy newer than $since belongs to the new process.
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 500
        if ((Test-Path $serverFile) -and (Get-Item $serverFile).LastWriteTimeUtc -ge $since -and (Get-RecordedServer)) {
            Write-Host ''
            Write-Host 'Ostra is running. Open this URL to sign in (it works once; run `ostra url` for another):'
            Write-Host ''
            Write-Host "  $(& $bin url)"
            Write-Host ''
            return
        }
    }
    throw "The service did not report a running server within 15 seconds. Check $dataDir\server.log and the last result in .\install.ps1 status."
}

function Install-Ostra([string[]]$flags) {
    Assert-Environment
    if ($env:SKIP_BUILD -ne '1') { Build-Ostra }
    $built = Join-Path $repo 'target\release\ostra.exe'
    if (-not (Test-Path $built)) { throw "Build $built first, because it is missing." }
    Stop-OstraService
    Assert-NoOtherServer
    Write-Step "Installing $bin"
    New-Item -ItemType Directory -Force $binDir, $dataDir | Out-Null
    # A running exe cannot be overwritten but can be renamed, so a copy started by hand keeps running.
    Remove-Item "$bin.old" -Force -ErrorAction SilentlyContinue
    if (Test-Path $bin) { Move-Item $bin "$bin.old" -Force }
    Copy-Item $built $bin
    Remove-Item "$bin.old" -Force -ErrorAction SilentlyContinue
    Write-Step "Registering the scheduled task $task"
    Register-OstraTask $flags
    Write-Step 'Starting the service'
    Start-OstraService
    $address = Get-BindAddress $flags
    if ($address -and $address -notin '127.0.0.1', '::1', 'localhost') {
        Write-Host 'Ostra listens on every interface over plain HTTP. Anyone who reaches the port with a sign-in link can'
        Write-Host 'run commands as you. Prefer an SSH tunnel or a TLS reverse proxy, or reinstall with --bind 127.0.0.1.'
    }
    $path = @($env:Path -split ';') + @((Get-SavedEnv 'Path') -split ';')
    if ($path -notcontains $binDir) {
        Write-Host "Add $binDir to PATH to run ``ostra url``, ``ostra stop``, and the other commands."
    }
}

function Restart-Ostra {
    if (-not (Get-OstraTask) -or -not (Test-Path $bin)) { throw 'Run .\install.ps1 first, because Ostra is not installed.' }
    Stop-OstraService
    Assert-NoOtherServer
    Start-OstraService
}

function Show-Status {
    $registered = Get-OstraTask
    if (-not $registered) { Write-Host 'The service is not installed.'; return }
    $info = Get-ScheduledTaskInfo -TaskName $task
    Write-Host "state = $($registered.State)"
    Write-Host "last run = $($info.LastRunTime)"
    Write-Host ('last result = 0x{0:X}' -f $info.LastTaskResult)
    $server = Get-RecordedServer
    if ($server) { Write-Host "pid = $($server.Id) ($($server.Path))" } else { Write-Host 'No server is running.' }
}

function Uninstall-Ostra {
    Write-Step 'Stopping the service'
    Stop-OstraService
    if (Get-OstraTask) { Unregister-ScheduledTask -TaskName $task -Confirm:$false }
    Remove-Item $bin, "$bin.old" -Force -ErrorAction SilentlyContinue
    Write-Host "Removed the service and $bin. The config file and the data in $dataDir are kept."
}

$command = if ($scriptArgs.Count -gt 0) { $scriptArgs[0] } else { 'install' }
$rest = [string[]]@($scriptArgs | Select-Object -Skip 1)
switch -Wildcard ($command) {
    'install' { Install-Ostra $rest }
    'restart' { Restart-Ostra }
    'status' { Show-Status }
    'url' { & $bin url; exit $LASTEXITCODE }
    'uninstall' { Uninstall-Ostra }
    '-*' { Install-Ostra ([string[]]$scriptArgs) }
    default { throw "Unknown command $command. Use install, restart, status, url, or uninstall." }
}
