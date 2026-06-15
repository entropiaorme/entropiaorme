# Pester unit test for the pure Caddy lifecycle decision helper. Dot-sources
# caddy-lib.ps1 so no live `caddy` or admin endpoint is touched: the
# start-vs-reload decision is exercised as a pure mapping. Written in
# Pester 3.x assertion syntax (`Should Be`), the version bundled with
# Windows PowerShell, so `Invoke-Pester` runs it with no extra install.
. (Join-Path $PSScriptRoot 'caddy-lib.ps1')

Describe 'Get-CaddyEnsureAction' {
    It 'reloads when Caddy is already running' {
        Get-CaddyEnsureAction -IsRunning $true | Should Be 'reload'
    }
    It 'starts when Caddy is down' {
        Get-CaddyEnsureAction -IsRunning $false | Should Be 'start'
    }
}
