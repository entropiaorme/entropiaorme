# Pure, side-effect-free helpers for Caddy lifecycle decisions, split out
# of caddy-lifecycle.ps1 so they can be unit-tested without the
# param-driven lifecycle body (and its live `caddy` calls) running on
# import. Dot-sourced by caddy-lifecycle.ps1 and by caddy-lib.Tests.ps1.

function Get-CaddyEnsureAction {
    # Decide how to bring Caddy to a running, current-config state: start
    # it when it is down, reload it when it is already up. Keeping this a
    # pure bool -> string mapping makes the idempotent-ensure behaviour
    # unit-testable without a live Caddy.
    param(
        [Parameter(Mandatory)]
        [bool]$IsRunning
    )
    if ($IsRunning) { 'reload' } else { 'start' }
}
