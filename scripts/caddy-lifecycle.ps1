param(
    [Parameter(Mandatory)]
    [ValidateSet('up', 'down', 'status', 'reload')]
    [string]$Action
)

$ErrorActionPreference = "Stop"

# Caddy lifecycle wrapper. Resolves the main worktree's Caddyfile via
# `git worktree list --porcelain` so reloads / starts triggered from any
# checkout of this repo target the same canonical config. This is what
# makes multi-checkout coexistence work: the canonical Caddyfile imports
# `.dev/Caddyfile.worktrees/*.caddy` relative to its own directory, so
# always loading it from the main worktree means every active checkout's
# per-checkout routing fragment in that directory survives any reload
# regardless of which checkout triggered it.
#
# Used by `just proxy-up` / `proxy-down` / `proxy-status` and by the
# defensive reload in `dev-launch.ps1`.

# Gate every action on Caddy being on PATH so a missing install surfaces
# a consistent install hint rather than an opaque "term not recognised"
# error from PowerShell.
if (-not (Get-Command caddy -ErrorAction SilentlyContinue)) {
    Write-Output "caddy not on PATH; install with 'winget install CaddyServer.Caddy' (see README Optional dev environment)."
    exit 1
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

# Resolve the main worktree path: the first entry from
# `git worktree list --porcelain` is the main worktree by git's
# documented contract. When invoked from the main worktree itself the
# first entry equals $repoRoot, so the resolution is idempotent. Falls
# back to $repoRoot on any failure (git missing, not a git repo,
# malformed output) so the script stays functional in degraded
# environments; the fallback just loses the cross-worktree-coexistence
# property for that invocation.
function Resolve-MainWorktree {
    try {
        $worktreeOutput = & git -C $repoRoot worktree list --porcelain 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $worktreeOutput) {
            return $repoRoot
        }
        foreach ($line in $worktreeOutput) {
            if ($line -match '^worktree\s+(.+)$') {
                return (Resolve-Path $Matches[1]).Path
            }
        }
        return $repoRoot
    } catch {
        return $repoRoot
    }
}

$mainWorktree = Resolve-MainWorktree
$caddyfile = Join-Path $mainWorktree "Caddyfile"

if (-not (Test-Path $caddyfile)) {
    Write-Output "main worktree Caddyfile not found at $caddyfile; cannot manage Caddy without a config."
    exit 1
}

switch ($Action) {
    'up' {
        & caddy start --config $caddyfile
    }
    'down' {
        & caddy stop
    }
    'status' {
        # Cheap liveness probe via Caddy's admin endpoint (default
        # localhost:2019). Mirrors the previous inline `just proxy-status`
        # shape; kept here so every lifecycle action routes through one
        # script.
        try {
            $null = Invoke-WebRequest -Uri "http://localhost:2019/config/" -UseBasicParsing -TimeoutSec 2 -ErrorAction Stop
            Write-Output "caddy running"
        } catch {
            Write-Output "caddy not running"
        }
    }
    'reload' {
        # `caddy reload` requires a running Caddy; if the admin endpoint
        # is unreachable, Caddy surfaces its own "admin endpoint
        # unreachable" stderr. We don't block on that; the dev launch
        # flow tolerates Caddy not running (the port-based devUrl
        # fallback keeps `just dev` working).
        & caddy reload --config $caddyfile
    }
}
