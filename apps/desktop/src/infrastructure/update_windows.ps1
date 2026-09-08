$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName System.IO.Compression.FileSystem

$root = $env:NEXUS_UPDATE_ROOT
$destination = $env:NEXUS_UPDATE_DESTINATION
$archive = [System.IO.Compression.ZipFile]::OpenRead($env:NEXUS_UPDATE_ARCHIVE)
try {
    if ($archive.Entries.Count -eq 0) { throw 'The update archive is empty' }
    foreach ($entry in $archive.Entries) {
        $name = $entry.FullName.Replace('\', '/').TrimEnd('/')
        $parts = $name.Split('/')
        $type = ($entry.ExternalAttributes -shr 16) -band 0xF000
        if ($name.StartsWith('/') -or $name.Contains(':') -or
            $parts -contains '' -or $parts -contains '.' -or $parts -contains '..' -or
            !($name -ceq $root -or $name.StartsWith($root + '/', [StringComparison]::Ordinal))) {
            throw "Unexpected path in update archive: $name"
        }
        if ($type -notin @(0, 0x8000, 0x4000) -or ($entry.ExternalAttributes -band 0x400)) {
            throw 'Links and special files are not allowed in update archives'
        }
    }
    foreach ($entry in $archive.Entries) {
        $target = [System.IO.Path]::Combine($destination, $entry.FullName.Replace('/', '\'))
        if ($entry.FullName.EndsWith('/') -or $entry.FullName.EndsWith('\')) {
            [void][System.IO.Directory]::CreateDirectory($target)
        } else {
            [void][System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($target))
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $false)
        }
    }
} finally {
    $archive.Dispose()
}
