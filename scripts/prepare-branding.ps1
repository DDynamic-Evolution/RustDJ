# Rebuild Windows icon sizes from the approved PNG, without changing the design.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$brandPath = Join-Path (Split-Path $PSScriptRoot -Parent) 'assets\branding'
$source = [System.Drawing.Image]::FromFile((Join-Path $brandPath 'rustdj-icon.png'))
try {
    $frames = @(foreach ($size in @(16, 20, 24, 32, 40, 48, 64, 128, 256)) {
        $bitmap = [System.Drawing.Bitmap]::new($size, $size)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $stream = [System.IO.MemoryStream]::new()
        try {
            $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
            $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $graphics.DrawImage($source, 0, 0, $size, $size)
            $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
            if ($size -eq 256) {
                [System.IO.File]::WriteAllBytes((Join-Path $brandPath 'rustdj-window-icon.png'), $stream.ToArray())
            }
            [PSCustomObject]@{ Size = $size; Data = $stream.ToArray() }
        } finally {
            $stream.Dispose()
            $graphics.Dispose()
            $bitmap.Dispose()
        }
    })
    $file = [System.IO.File]::Create((Join-Path $brandPath 'rustdj.ico'))
    $writer = [System.IO.BinaryWriter]::new($file)
    try {
        $writer.Write([uint16]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]$frames.Count)
        $offset = 6 + 16 * $frames.Count
        foreach ($frame in $frames) {
            $dimension = if ($frame.Size -eq 256) { 0 } else { $frame.Size }
            $writer.Write([byte]$dimension)
            $writer.Write([byte]$dimension)
            $writer.Write([byte]0)
            $writer.Write([byte]0)
            $writer.Write([uint16]1)
            $writer.Write([uint16]32)
            $writer.Write([uint32]$frame.Data.Length)
            $writer.Write([uint32]$offset)
            $offset += $frame.Data.Length
        }
        foreach ($frame in $frames) { $writer.Write([byte[]]$frame.Data) }
    } finally { $writer.Dispose() }
} finally { $source.Dispose() }
Write-Output 'Created rustdj-window-icon.png and rustdj.ico (16, 20, 24, 32, 40, 48, 64, 128, 256 px)'
