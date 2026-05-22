#!/usr/bin/env pwsh
$ErrorActionPreference = "Stop"

Set-Location -Path $PSScriptRoot

Write-Host "==> Installing dependencies..."
npm install

Write-Host "==> Building Tauri app..."
npx tauri build @Args

Write-Host "==> Done. Output in src-tauri/target/release/bundle/"
