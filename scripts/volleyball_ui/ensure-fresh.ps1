# Volley UI lifecycle. Exit 0 = reused/restarted, 1 = no server (launch),
# 2 = active processing (do not interrupt), 3 = restart failed (do not duplicate).
param([switch]$Restart)
$ErrorActionPreference = "Stop"
$base = "http://127.0.0.1:8765"

function Get-VolleyHealth {
    try { return Invoke-RestMethod "$base/api/health" -TimeoutSec 2 }
    catch { return $null }
}

$before = Get-VolleyHealth
if (-not $before) {
    # The listener can be briefly unavailable while Python re-execs itself.
    for ($attempt = 0; $attempt -lt 12 -and -not $before; $attempt++) {
        Start-Sleep -Milliseconds 250
        $before = Get-VolleyHealth
    }
}
if (-not $before) {
    # A listening port with no health reply is NOT permission to launch a clone.
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $connection = $client.BeginConnect("127.0.0.1", 8765, $null, $null)
        if ($connection.AsyncWaitHandle.WaitOne(700)) {
            try {
                $client.EndConnect($connection)
                Write-Host "Volley Studio: port 8765 is occupied; refusing a duplicate."
                exit 3
            } catch { }
        }
    } catch { } finally { $client.Close() }
    exit 1
}

if (-not $Restart -and $before.ok -and $before.api_version -ge 28 -and
    -not $before.reload_required -and -not $before.restarting) {
    Write-Host "Volley Studio: using existing server $($before.instance_id)."
    exit 0
}

try {
    $job = Invoke-RestMethod "$base/api/job" -TimeoutSec 3
    if ($job.status -in @('running', 'queued', 'starting', 'cancelling')) {
        Write-Host "Volley Studio: processing is active; restart blocked."
        exit 2
    }
} catch {
    Write-Host "Volley Studio: could not verify job status; leaving server untouched."
    exit 3
}

# The backend re-execs the same process, keeping exactly one listener.
# Old instances auto-restart when server.py changes; never kill by process name.
if ($before.api_version -ge 28 -and -not $before.restarting) {
    try {
        $body = Invoke-RestMethod "$base/api/system/restart" -Method Post -ContentType "application/json" -Body '{}' -TimeoutSec 4
        if (-not $body.accepted) { throw "restart not accepted" }
    } catch {
        Write-Host "Volley Studio: restart request interrupted; checking for server recovery."
    }
}

for ($attempt = 0; $attempt -lt 80; $attempt++) {
    Start-Sleep -Milliseconds 250
    $after = Get-VolleyHealth
    if (-not $after) { continue }
    if ($after.ok -and $after.api_version -ge 28 -and
        -not $after.reload_required -and -not $after.restarting -and
        $after.instance_id -and $after.instance_id -ne $before.instance_id) {
        Write-Host "Volley Studio: restarted successfully; one server on port 8765."
        exit 0
    }
}
Write-Host "Volley Studio: restart unconfirmed; NOT starting another instance."
exit 3
