<#
.SYNOPSIS
  Run any cargo command inside a Windows Job Object capped at ~66% of RAM and CPU.

.DESCRIPTION
  Windows has no ulimit. A Job Object is the kernel's equivalent: every process
  started inside it (cargo, rustc, build scripts, link.exe, test binaries, the
  lean-ctx binary that tests spawn, and their children) is counted and limited
  together.

    memory  JOB_OBJECT_LIMIT_JOB_MEMORY: hard ceiling on the COMMITTED memory of
            the whole tree. When it is reached the allocating process does not
            slow down: its allocation fails. rustc and Rust test binaries then
            abort ("memory allocation of N bytes failed", exit 0xc0000409),
            link.exe stops with LNK1102. The script counts those refusals and
            says so at the end.
    cpu     JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP: the tree gets at most CpuRate
            (1/100th of a percent of the whole machine) per scheduling interval.
    prio    BelowNormal by default. Windows children inherit BelowNormal and Idle,
            so the desktop wins every scheduling decision against the build.
    close   JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: whatever is still alive when the
            run ends (a daemon a test left behind) is killed with the job.
    crash   JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION: no Windows Error
            Reporting dialog blocks the run when a test binary crashes.

  Breakaway is NOT allowed, on purpose: sobre's spawn_detached
  (src/ipc/process.rs) first asks for CREATE_BREAKAWAY_FROM_JOB, gets
  ACCESS_DENIED here, and falls back to a plain detached spawn - so the
  proxies and daemons that tests start stay inside the cap.

  The script puts ITSELF in the job before it starts cargo, so nothing can
  escape between process creation and assignment. A process can never leave a
  job, so when called from an interactive PowerShell the script first re-runs
  itself in a fresh powershell.exe; your shell is never put in the job.

.NOTES
  Prefer the .cmd shim: it works from cmd, PowerShell and Git Bash, it keeps a
  bare '--' (PowerShell strips it before a .ps1 sees it), and it does not depend
  on the execution policy (Restricted by default on Windows 11 Home).

    scripts\cargo-capped.cmd test --lib
    scripts\cargo-capped.cmd test --test main -- --nocapture
    scripts\cargo-capped.cmd build --release

  cargo-capped.cmd, saved next to this file (two lines):
    @powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0cargo-capped.ps1" __sobre_cap_inner__ %*
    @exit /b %ERRORLEVEL%

  Settings - environment variables, all optional, no parameters (a param block
  would let PowerShell swallow cargo's -p / -j / -F flags):

    SOBRE_CAP_MEM_GB          committed-memory ceiling for the whole tree.
                              Default: 66% of physical RAM. 0 = no memory cap.
    SOBRE_CAP_CPU_PCT         hard CPU cap, % of ALL logical CPUs. Default 66.
                              100 = no CPU cap (priority still applies).
    SOBRE_CAP_PRIORITY        Normal | BelowNormal | Idle. Default BelowNormal.
    SOBRE_CAP_AFFINITY        optional hex mask of logical CPUs, e.g. 3F keeps
                              logical CPUs 6-7 (one whole core) for the desktop.
    SOBRE_CAP_JOBS            CARGO_BUILD_JOBS if you have not set it. Default 3.
                              Note: this env var overrides [build] jobs in
                              .cargo/config.toml; -j on the command line wins.
    SOBRE_CAP_TEST_THREADS    RUST_TEST_THREADS for test/bench if unset. Default 4.
                              --test-threads after '--' still wins.
    SOBRE_CAP_RETRY_SERIAL    1 (default): when a build-only command (build,
                              check, clippy, doc, test --no-run) failed because
                              the job refused memory, rerun it once with
                              CARGO_BUILD_JOBS=1. Cargo keeps finished units.
    SOBRE_CAP_OFF             1 = no job at all, priority only.
    SOBRE_CAP_EXE             program to run. Default cargo.

  For test/bench, PROPTEST_CASES=64 and LEAN_CTX_EMBEDDINGS_AUTO_DOWNLOAD=0 are
  also set when unset - the values CI and scripts/preflight.sh use. Every value
  the script sets is printed before cargo starts.
#>

$ErrorActionPreference = 'Stop'
$Sentinel = '__sobre_cap_inner__'

function Say([string]$Message) { [Console]::Error.WriteLine("cargo-capped: $Message") }

function EnvOr([string]$Name, [string]$Default) {
    $v = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($v)) { return $Default }
    return $v.Trim()
}

function ToDouble([string]$Text) {
    # Accept 5.2 and 5,2 alike (French locale).
    return [double]::Parse($Text.Replace(',', '.'), [Globalization.CultureInfo]::InvariantCulture)
}

# ------------------------------------------------------------------ stage 1
# Called directly from an interactive shell: re-run in a dedicated process.
if ($args.Count -eq 0 -or $args[0] -ne $Sentinel) {
    $line = $MyInvocation.Line
    if ($line -and ($line -match '(^|\s)--(\s|$)') -and -not ($args -contains '--')) {
        Say "PowerShell removed the bare '--' before this script could see it, so cargo would read the test flags as its own."
        Say "Quote it ('--'), or use scripts\cargo-capped.cmd, which keeps it."
        exit 2
    }
    $self = (Get-Process -Id $PID).Path
    & $self -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $PSCommandPath $Sentinel @args
    exit $LASTEXITCODE
}

# ------------------------------------------------------------------ stage 2
$cargoArgs = @()
if ($args.Count -gt 1) { $cargoArgs = @($args[1..($args.Count - 1)]) }

if (-not [Environment]::Is64BitProcess) {
    Say 'run this from 64-bit PowerShell: a 32-bit process cannot set a job memory limit above 4 GB.'
    exit 3
}

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class SobreCap
{
    [StructLayout(LayoutKind.Sequential)]
    struct JOBOBJECT_BASIC_LIMIT_INFORMATION
    {
        public long PerProcessUserTimeLimit;
        public long PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass;
        public uint SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct IO_COUNTERS
    {
        public ulong ReadOperationCount, WriteOperationCount, OtherOperationCount;
        public ulong ReadTransferCount, WriteTransferCount, OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    {
        public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        public IO_COUNTERS IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct JOBOBJECT_CPU_RATE_CONTROL_INFORMATION
    {
        public uint ControlFlags;
        public uint CpuRate; // union { CpuRate; Weight; MinRate/MaxRate } - HARD_CAP reads CpuRate
    }

    [StructLayout(LayoutKind.Sequential)]
    struct JOBOBJECT_BASIC_ACCOUNTING_INFORMATION
    {
        public long TotalUserTime, TotalKernelTime, ThisPeriodTotalUserTime, ThisPeriodTotalKernelTime;
        public uint TotalPageFaultCount, TotalProcesses, ActiveProcesses, TotalTerminatedProcesses;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct JOBOBJECT_ASSOCIATE_COMPLETION_PORT
    {
        public IntPtr CompletionKey;
        public IntPtr CompletionPort;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct MEMORYSTATUSEX
    {
        public uint dwLength, dwMemoryLoad;
        public ulong ullTotalPhys, ullAvailPhys, ullTotalPageFile, ullAvailPageFile;
        public ulong ullTotalVirtual, ullAvailVirtual, ullAvailExtendedVirtual;
    }

    const int JobObjectBasicAccountingInformation = 1;
    const int JobObjectAssociateCompletionPortInformation = 7;
    const int JobObjectExtendedLimitInformation = 9;
    const int JobObjectCpuRateControlInformation = 15;

    const uint JOB_OBJECT_LIMIT_AFFINITY = 0x00000010;
    const uint JOB_OBJECT_LIMIT_JOB_MEMORY = 0x00000200;
    const uint JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION = 0x00000400;
    const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
    // Deliberately absent: JOB_OBJECT_LIMIT_BREAKAWAY_OK (0x800) and
    // JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK (0x1000) - with either, daemons
    // spawned by tests would leave the cap.

    const uint JOB_OBJECT_CPU_RATE_CONTROL_ENABLE = 0x1;
    const uint JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP = 0x4;

    const uint JOB_OBJECT_MSG_JOB_MEMORY_LIMIT = 10;

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateJobObjectW(IntPtr lpJobAttributes, string lpName);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr hJob, int infoClass, IntPtr info, uint length);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool QueryInformationJobObject(IntPtr hJob, int infoClass, IntPtr info, uint length, IntPtr returnLength);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);

    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr CreateIoCompletionPort(IntPtr fileHandle, IntPtr existingPort, UIntPtr key, uint threads);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetQueuedCompletionStatus(IntPtr port, out uint bytes, out UIntPtr key, out IntPtr overlapped, uint milliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX status);

    static IntPtr job = IntPtr.Zero;
    static IntPtr port = IntPtr.Zero;

    static void Set(int infoClass, object value)
    {
        int size = Marshal.SizeOf(value);
        IntPtr buffer = Marshal.AllocHGlobal(size);
        try
        {
            Marshal.StructureToPtr(value, buffer, false);
            if (!SetInformationJobObject(job, infoClass, buffer, (uint)size))
                throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }

    static object Query(int infoClass, Type type)
    {
        int size = Marshal.SizeOf(type);
        IntPtr buffer = Marshal.AllocHGlobal(size);
        try
        {
            if (!QueryInformationJobObject(job, infoClass, buffer, (uint)size, IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            return Marshal.PtrToStructure(buffer, type);
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }

    // jobMemoryBytes = 0: no memory cap. cpuRate in 1/100th of a percent of the
    // whole machine, 0 or >= 10000: no CPU cap. affinityMask = 0: all CPUs.
    public static void Create(ulong jobMemoryBytes, uint cpuRate, ulong affinityMask)
    {
        job = CreateJobObjectW(IntPtr.Zero, null);
        if (job == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());

        var limits = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
        uint flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        if (jobMemoryBytes > 0)
        {
            flags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
            limits.JobMemoryLimit = new UIntPtr(jobMemoryBytes);
        }
        if (affinityMask > 0)
        {
            flags |= JOB_OBJECT_LIMIT_AFFINITY;
            limits.BasicLimitInformation.Affinity = new UIntPtr(affinityMask);
        }
        limits.BasicLimitInformation.LimitFlags = flags;
        Set(JobObjectExtendedLimitInformation, limits);

        if (cpuRate > 0 && cpuRate < 10000)
        {
            var rate = new JOBOBJECT_CPU_RATE_CONTROL_INFORMATION();
            rate.ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
            rate.CpuRate = cpuRate;
            Set(JobObjectCpuRateControlInformation, rate);
        }

        // Completion port: the kernel posts JOB_OBJECT_MSG_JOB_MEMORY_LIMIT each
        // time it refuses an allocation, so the report can say so for certain.
        port = CreateIoCompletionPort(new IntPtr(-1), IntPtr.Zero, UIntPtr.Zero, 1);
        if (port == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        var association = new JOBOBJECT_ASSOCIATE_COMPLETION_PORT();
        association.CompletionKey = job;
        association.CompletionPort = port;
        Set(JobObjectAssociateCompletionPortInformation, association);
    }

    // Puts this PowerShell process in the job. Every process it starts from
    // now on is created inside the job, so nothing can escape before assignment.
    public static void AssignSelf()
    {
        if (!AssignProcessToJobObject(job, GetCurrentProcess()))
            throw new Win32Exception(Marshal.GetLastWin32Error());
    }

    public static int DrainMemoryLimitHits()
    {
        if (port == IntPtr.Zero) return 0;
        int hits = 0;
        uint message; UIntPtr key; IntPtr processId;
        while (GetQueuedCompletionStatus(port, out message, out key, out processId, 0))
            if (message == JOB_OBJECT_MSG_JOB_MEMORY_LIMIT) hits++;
        return hits;
    }

    public sealed class Usage
    {
        public ulong PeakJobBytes;
        public double CpuSeconds;
        public uint Processes;
        public uint StillRunning;
    }

    public static Usage Read()
    {
        var u = new Usage();
        if (job == IntPtr.Zero) return u;
        var ext = (JOBOBJECT_EXTENDED_LIMIT_INFORMATION)Query(JobObjectExtendedLimitInformation, typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
        var acc = (JOBOBJECT_BASIC_ACCOUNTING_INFORMATION)Query(JobObjectBasicAccountingInformation, typeof(JOBOBJECT_BASIC_ACCOUNTING_INFORMATION));
        u.PeakJobBytes = ext.PeakJobMemoryUsed.ToUInt64();
        u.CpuSeconds = (acc.TotalUserTime + acc.TotalKernelTime) / 1e7;
        u.Processes = acc.TotalProcesses;
        u.StillRunning = acc.ActiveProcesses;
        return u;
    }

    // [0] = total physical bytes, [1] = available now (free + standby).
    public static ulong[] PhysicalMemory()
    {
        var m = new MEMORYSTATUSEX();
        m.dwLength = (uint)Marshal.SizeOf(typeof(MEMORYSTATUSEX));
        if (!GlobalMemoryStatusEx(ref m)) throw new Win32Exception(Marshal.GetLastWin32Error());
        return new ulong[] { m.ullTotalPhys, m.ullAvailPhys };
    }
}
'@

# ------------------------------------------------------------------ settings
$phys = [SobreCap]::PhysicalMemory()
$totalBytes = [double]$phys[0]
$availBytes = [double]$phys[1]

$memGB = [math]::Round($totalBytes * 0.66 / 1GB, 2)
$memText = EnvOr 'SOBRE_CAP_MEM_GB' ''
if ($memText) { $memGB = ToDouble $memText }
$memBytes = [uint64]([math]::Max(0.0, $memGB) * 1GB)

$cpuPct = ToDouble (EnvOr 'SOBRE_CAP_CPU_PCT' '66')
$cpuRate = [uint32][math]::Max(0.0, [math]::Round($cpuPct * 100))
if ($cpuRate -ge 10000) { $cpuRate = 0 }

$priority = [Diagnostics.ProcessPriorityClass](EnvOr 'SOBRE_CAP_PRIORITY' 'BelowNormal')

$affinity = [uint64]0
$affText = EnvOr 'SOBRE_CAP_AFFINITY' ''
if ($affText) { $affinity = [Convert]::ToUInt64($affText, 16) }

$exe = EnvOr 'SOBRE_CAP_EXE' 'cargo'
$sub = @($cargoArgs | Where-Object { $_ -notlike '+*' } | Select-Object -First 1)
$sub = if ($sub.Count -gt 0) { [string]$sub[0] } else { '' }
$isTest = @('test', 't', 'bench', 'nextest') -contains $sub
$buildOnly = (@('build', 'b', 'check', 'c', 'clippy', 'doc', 'd', 'rustc') -contains $sub) -or ($cargoArgs -contains '--no-run')
$userJobs = @($cargoArgs | Where-Object { $_ -match '^(-j|--jobs)' }).Count -gt 0

$defaults = [ordered]@{ CARGO_BUILD_JOBS = (EnvOr 'SOBRE_CAP_JOBS' '3') }
if ($isTest) {
    $defaults['RUST_TEST_THREADS'] = (EnvOr 'SOBRE_CAP_TEST_THREADS' '4')
    $defaults['PROPTEST_CASES'] = '64'
    $defaults['LEAN_CTX_EMBEDDINGS_AUTO_DOWNLOAD'] = '0'
}
$shown = @()
foreach ($name in $defaults.Keys) {
    $current = [Environment]::GetEnvironmentVariable($name)
    if ([string]::IsNullOrEmpty($current)) {
        [Environment]::SetEnvironmentVariable($name, [string]$defaults[$name])
        $shown += "$name=$($defaults[$name])"
    } else {
        $shown += "$name=$current (yours)"
    }
}

$capOff = (EnvOr 'SOBRE_CAP_OFF' '0') -eq '1'
$memLabel = if ($memBytes -gt 0) { '{0:N1} GB' -f $memGB } else { 'none' }
$cpuLabel = if ($cpuRate -gt 0) { '{0:N0}%' -f ($cpuRate / 100) } else { 'none' }
if ($capOff) { $memLabel = 'off'; $cpuLabel = 'off' }
Say ("caps: memory {0} (machine {1:N1} GB, {2:N1} GB available now), cpu {3}, priority {4}{5}" -f `
        $memLabel, ($totalBytes / 1GB), ($availBytes / 1GB), $cpuLabel, $priority,
        $(if ($affinity) { ', affinity 0x{0:X}' -f $affinity } else { '' }))
Say ("env:  " + ($shown -join '  '))
if (-not $capOff -and $memBytes -gt $availBytes) {
    Say ("warning: only {0:N1} GB is available right now. The cap limits cargo; it does not free memory, so Windows will page until other programs give some back." -f ($availBytes / 1GB))
}

# ------------------------------------------------------------------ job
if (-not $capOff) {
    try {
        [SobreCap]::Create($memBytes, $cpuRate, $affinity)
        [SobreCap]::AssignSelf()
    } catch {
        Say "could not create or join the job object: $($_.Exception.Message)"
        Say 'nothing was started. Set SOBRE_CAP_OFF=1 to run with priority only.'
        exit 3
    }
}
# BelowNormal and Idle are inherited by every child (CreateProcess rule).
[Diagnostics.Process]::GetCurrentProcess().PriorityClass = $priority

if (-not (Get-Command $exe -ErrorAction SilentlyContinue)) {
    Say "'$exe' was not found on PATH."
    exit 127
}

# ------------------------------------------------------------------ run
$ErrorActionPreference = 'Continue' # native stderr must never become a terminating error
$code = 1
$hits = 0
# Job accounting includes this PowerShell's own CPU time since it started
# (Add-Type compiles C#); measure from here.
$cpuBefore = [SobreCap]::Read().CpuSeconds
$clock = [Diagnostics.Stopwatch]::StartNew()
try {
    $global:LASTEXITCODE = 0
    & $exe @cargoArgs
    $code = $LASTEXITCODE
    $hits = [SobreCap]::DrainMemoryLimitHits()

    $retry = (EnvOr 'SOBRE_CAP_RETRY_SERIAL' '1') -eq '1'
    if ($code -ne 0 -and $hits -gt 0 -and $retry -and $buildOnly -and -not $userJobs -and $env:CARGO_BUILD_JOBS -ne '1') {
        Say "the job refused $hits allocation(s) at its $memLabel cap. Cargo keeps every unit it finished; retrying once with CARGO_BUILD_JOBS=1."
        $env:CARGO_BUILD_JOBS = '1'
        $global:LASTEXITCODE = 0
        & $exe @cargoArgs
        $code = $LASTEXITCODE
        # Only the attempt whose exit code is reported may explain it: a retry
        # that fails on a compile error must not be blamed on the first run's
        # refusals.
        $hits = [SobreCap]::DrainMemoryLimitHits()
    }
} finally {
    $u = [SobreCap]::Read()
    $wall = $clock.Elapsed
    if (-not $capOff) {
        $cpuSeconds = [math]::Max(0.0, $u.CpuSeconds - $cpuBefore)
        $avgCpus = if ($wall.TotalSeconds -gt 0) { $cpuSeconds / $wall.TotalSeconds } else { 0 }
        Say ("exit {0} after {1:hh\:mm\:ss} | peak commit {2:N2} GB of {3} | CPU {4:N1} s, {5:N1} of {6} logical CPUs on average | {7} processes" -f `
                $code, $wall, ($u.PeakJobBytes / 1GB), $memLabel, $cpuSeconds, $avgCpus, [Environment]::ProcessorCount, $u.Processes)
        if ($hits -gt 0) {
            Say "the job refused $hits allocation(s) in this run: an out-of-memory failure above comes from the cap, not from the code. Lower SOBRE_CAP_JOBS or SOBRE_CAP_TEST_THREADS, or raise SOBRE_CAP_MEM_GB. (The peak above includes the refused request, so it can exceed the cap.)"
        }
        $stray = [int]$u.StillRunning - 1 # minus this PowerShell
        if ($stray -gt 0) {
            Say "$stray process(es) were still running in the job (left behind by the run); they die with the job now."
        }
    }
}
exit $code

