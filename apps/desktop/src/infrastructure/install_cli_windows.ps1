$ErrorActionPreference = 'Stop'
$directory = $env:NEXUS_CLI_BIN
if ([string]::IsNullOrWhiteSpace($directory)) { throw 'CLI directory is missing' }

Add-Type -Namespace NexusCli -Name EnvironmentNotification -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr window, uint message,
    System.UIntPtr wParam, string lParam, uint flags, uint timeout, out System.UIntPtr result);
'@

$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
try {
    # Preserve expandable entries and the complete existing user PATH.
    $current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $present = @($current -split ';' | Where-Object {
        [Environment]::ExpandEnvironmentVariables($_.Trim().Trim('"')).TrimEnd('\').Equals(
            $directory.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)
    }).Count -gt 0
    if (-not $present) {
        $updated = if ([string]::IsNullOrEmpty($current)) { $directory } else { "$directory;$current" }
        $key.SetValue('Path', $updated, [Microsoft.Win32.RegistryValueKind]::ExpandString)
    }
} finally {
    $key.Dispose()
}

$result = [UIntPtr]::Zero
[void][NexusCli.EnvironmentNotification]::SendMessageTimeout(
    [IntPtr]0xffff, 0x001a, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
