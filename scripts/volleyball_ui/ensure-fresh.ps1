# Check whether the existing WSL server matches the current files.
# Exit 0 = reuse; 1 = start new; 2 = running job, keep existing server.
$ErrorActionPreference = 'Stop'
$address = 'http://127.0.0.1:8765'
try {
    $health = Invoke-RestMethod "$address/api/health" -TimeoutSec 3
    if ($health.ok -and $health.api_version -ge 3 -and -not $health.reload_required) {
        exit 0
    }
} catch {
    # No server or outdated server without the health endpoint.
}
try {
    $job = Invoke-RestMethod "$address/api/job" -TimeoutSec 3
    if ($job.status -in @('running', 'queued', 'starting', 'cancelling')) {
        Write-Host 'Analysis is active. Restart Volley Studio after the job completes.'
        exit 2
    }
    # Only stop the dedicated UI interpreter. Do not touch analysis workers.
    & wsl.exe -d Ubuntu-25.04 -e bash -lc "pkill -TERM -f -x '.venv/bin/python -m scripts.volleyball_ui' || true" | Out-Null
    Start-Sleep -Milliseconds 600
} catch {
    # No previous server found.
}
exit 1
