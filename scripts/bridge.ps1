[CmdletBinding()]
param(
    [ValidateSet('start', 'stop', 'status')]
    [string]$Action = 'status',
    [int]$Port = 0,
    [string]$Exe = '',
    [int]$TimeoutSeconds = 15,
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'

function Resolve-Port {
    if ($Port -gt 0) { return $Port }
    if ($env:EDGE_MCP_PORT -match '^\d+$') {
        $value = [int]$env:EDGE_MCP_PORT
        if ($value -ge 1 -and $value -le 65535) { return $value }
    }
    return 8765
}

# 纯函数：用于自检“相似端口不匹配”和“可执行文件身份”。
function Test-PortMatches([int]$ListenerPort, [int]$WantedPort) {
    return $ListenerPort -eq $WantedPort
}

function Test-SameExecutablePath([string]$Actual, [string]$Expected) {
    if ([string]::IsNullOrWhiteSpace($Actual) -or [string]::IsNullOrWhiteSpace($Expected)) {
        return $false
    }
    try {
        $actualPath = [IO.Path]::GetFullPath($Actual)
        $expectedPath = [IO.Path]::GetFullPath($Expected)
        return $actualPath -ieq $expectedPath
    } catch {
        return $false
    }
}

function Assert-True([bool]$Condition, [string]$Name) {
    if (-not $Condition) { throw "self-test failed: $Name" }
}

function Invoke-SelfTest {
    Assert-True (Test-PortMatches 8765 8765) 'exact port matches'
    Assert-True (-not (Test-PortMatches 87650 8765)) 'similar port must not match'
    Assert-True (-not (Test-PortMatches 18765 8765)) 'prefixed port must not match'
    Assert-True (Test-SameExecutablePath 'C:\EMA2\bin\ema2.exe' 'c:/EMA2/bin/ema2.exe') 'path comparison is case/separator tolerant'
    Assert-True (-not (Test-SameExecutablePath 'C:\Other\ema2.exe' 'C:\EMA2\bin\ema2.exe')) 'foreign executable differs'
    Assert-True (-not (Test-SameExecutablePath '' 'C:\EMA2\bin\ema2.exe')) 'missing executable path is not a match'
    Write-Output 'bridge.ps1 self-test ok'
}

function Get-ExactListeners([int]$WantedPort) {
    # 先取全部监听再按整数精确过滤，避免端口字符串的子串误判。
    try {
        $listeners = @(Get-NetTCPConnection -State Listen -ErrorAction Stop)
    } catch {
        [Console]::Error.WriteLine("cannot query TCP listeners: $($_.Exception.Message)")
        exit 6
    }
    return @($listeners | Where-Object { Test-PortMatches ([int]$_.LocalPort) $WantedPort })
}

function Get-ProcessPath([int]$ProcessId) {
    try {
        return (Get-Process -Id $ProcessId -ErrorAction Stop).Path
    } catch {
        return ''
    }
}

function Test-Ema2Listener($Listener, [string]$ExpectedExe) {
    $path = Get-ProcessPath ([int]$Listener.OwningProcess)
    return Test-SameExecutablePath $path $ExpectedExe
}

function Write-ListenerReport($Listener, [string]$ExpectedExe) {
    $path = Get-ProcessPath ([int]$Listener.OwningProcess)
    if ([string]::IsNullOrWhiteSpace($path)) { $path = '<unknown>' }
    [Console]::Error.WriteLine("port is owned by PID $($Listener.OwningProcess) ($path), not $ExpectedExe; refusing to stop it.")
}

function Test-Health([int]$WantedPort) {
    try {
        $headers = @{}
        if ($env:EDGE_MCP_TOKEN) {
            $headers['Authorization'] = "Bearer $($env:EDGE_MCP_TOKEN)"
        }
        $status = Invoke-RestMethod -Uri "http://127.0.0.1:$WantedPort/status" -Headers $headers -TimeoutSec 3
        return ($status.name -eq 'ema2')
    } catch {
        return $false
    }
}

if ($SelfTest) {
    Invoke-SelfTest
    exit 0
}

$resolvedPort = Resolve-Port
if ([string]::IsNullOrWhiteSpace($Exe)) {
    $Exe = Join-Path (Split-Path -Parent $PSScriptRoot) 'bin\ema2.exe'
}
$exePath = [IO.Path]::GetFullPath($Exe)

if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) {
    [Console]::Error.WriteLine("EMA2 executable not found: $exePath")
    exit 1
}

$listeners = @(Get-ExactListeners $resolvedPort)

switch ($Action) {
    'status' {
        if ($listeners.Count -eq 0) {
            Write-Output "EMA2 is NOT running on port $resolvedPort."
            exit 1
        }
        foreach ($listener in $listeners) {
            if (-not (Test-Ema2Listener $listener $exePath)) {
                Write-ListenerReport $listener $exePath
                exit 2
            }
        }
        if (Test-Health $resolvedPort) {
            Write-Output "EMA2 is running on port $resolvedPort."
            exit 0
        }
        Write-Output "EMA2 is listening on port $resolvedPort but /status is not healthy."
        exit 3
    }
    'start' {
        if ($listeners.Count -gt 0) {
            foreach ($listener in $listeners) {
                if (-not (Test-Ema2Listener $listener $exePath)) {
                    Write-ListenerReport $listener $exePath
                    exit 2
                }
            }
            if (Test-Health $resolvedPort) {
                Write-Output "EMA2 already running on port $resolvedPort (verified /status)."
                exit 0
            }
            [Console]::Error.WriteLine("EMA2 owns port $resolvedPort but /status is not healthy; not starting a second copy.")
            exit 3
        }

        $process = Start-Process -FilePath $exePath -ArgumentList @('run', '--port', "$resolvedPort") -WindowStyle Hidden -PassThru
        $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
        while ((Get-Date) -lt $deadline) {
            Start-Sleep -Milliseconds 250
            $current = @(Get-ExactListeners $resolvedPort)
            if ($current.Count -gt 0) {
                $foreign = @($current | Where-Object { -not (Test-Ema2Listener $_ $exePath) })
                if ($foreign.Count -gt 0) {
                    Write-ListenerReport $foreign[0] $exePath
                    exit 2
                }
                if (Test-Health $resolvedPort) {
                    Write-Output "EMA2 started on port $resolvedPort (MCP http://127.0.0.1:$resolvedPort/mcp, PID $($process.Id))."
                    exit 0
                }
            }
            if ($process.HasExited) {
                [Console]::Error.WriteLine("EMA2 exited during startup with code $($process.ExitCode).")
                exit 4
            }
        }
        [Console]::Error.WriteLine("EMA2 did not become healthy on port $resolvedPort within ${TimeoutSeconds}s.")
        exit 4
    }
    'stop' {
        if ($listeners.Count -eq 0) {
            Write-Output "EMA2 is not running on port $resolvedPort."
            exit 0
        }
        foreach ($listener in $listeners) {
            if (-not (Test-Ema2Listener $listener $exePath)) {
                Write-ListenerReport $listener $exePath
                exit 2
            }
        }
        $pids = @($listeners | ForEach-Object { [int]$_.OwningProcess } | Select-Object -Unique)
        foreach ($processId in $pids) {
            Stop-Process -Id $processId -Force -ErrorAction Stop
        }
        Start-Sleep -Milliseconds 300
        $remaining = @(Get-ExactListeners $resolvedPort)
        if ($remaining.Count -gt 0) {
            [Console]::Error.WriteLine("EMA2 is still listening on port $resolvedPort after stop.")
            exit 5
        }
        Write-Output "EMA2 stopped (port $resolvedPort)."
        exit 0
    }
}
