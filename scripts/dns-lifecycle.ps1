param(
    [Parameter(Mandatory)]
    [ValidateSet('up', 'down', 'status')]
    [string]$Action
)

$ErrorActionPreference = "Stop"

# CoreDNS doesn't ship a daemonising `start`/`stop` subcommand the way
# Caddy does, so the lifecycle is managed here: launch detached via
# Start-Process, identify by process name for stop/status. Used by
# `just dns-up` / `dns-down` / `dns-status`.

# Gate every action on CoreDNS being on PATH so a missing install
# surfaces a consistent install hint rather than an opaque
# "term not recognised" error from PowerShell.
if (-not (Get-Command coredns -ErrorAction SilentlyContinue)) {
    Write-Output "coredns not on PATH; install with 'scoop install coredns' (see README Prerequisites)."
    exit 1
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$corefile = Join-Path $repoRoot "Corefile"
$logDir = Join-Path $repoRoot ".dev"
$logOut = Join-Path $logDir "coredns.out.log"
$logErr = Join-Path $logDir "coredns.err.log"

switch ($Action) {
    'up' {
        if (Get-Process coredns -ErrorAction SilentlyContinue) {
            Write-Output "coredns already running"
            return
        }
        if (-not (Test-Path $logDir)) {
            New-Item -ItemType Directory -Path $logDir | Out-Null
        }
        # Start detached. Stdout/stderr capture goes to .dev/coredns.*.log so
        # bind failures or Corefile syntax errors are diagnosable even with
        # the console window hidden; logs are overwritten on each start.
        $proc = Start-Process coredns `
            -ArgumentList '-conf', $corefile `
            -WindowStyle Hidden `
            -PassThru `
            -RedirectStandardOutput $logOut `
            -RedirectStandardError $logErr
        # Brief settle so a bind failure or config error shows up before we
        # claim success. CoreDNS exits within milliseconds when it can't
        # bind or the Corefile is malformed.
        Start-Sleep -Milliseconds 500
        if ($proc.HasExited) {
            Write-Output "coredns failed to start (exit $($proc.ExitCode)); see .dev/coredns.err.log or run 'coredns -conf Corefile' in a foreground shell."
            exit 1
        }
        Write-Output "coredns started"
    }
    'down' {
        $procs = Get-Process coredns -ErrorAction SilentlyContinue
        if ($procs) {
            $procs | Stop-Process -Force
            Write-Output "coredns stopped"
        } else {
            Write-Output "coredns not running"
        }
    }
    'status' {
        if (Get-Process coredns -ErrorAction SilentlyContinue) {
            Write-Output "coredns running"
        } else {
            Write-Output "coredns not running"
        }
    }
}
