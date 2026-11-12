import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

export function runShortcutChecks(requireFromWork, desktopContext) {
  const {
    workDir, test, skip, expectEqual, assert, expectExit,
  } = desktopContext;
  // --- Windows Start Menu shortcut (issue #2053) ---
  const shortcutMod = requireFromWork(path.join(workDir, 'windows-shortcut.js'));
  const globalWinEnv = { npm_config_global: 'true', APPDATA: path.join(workDir, 'appdata') };

  test('S1', () => {
    expectEqual(shortcutMod.skipReason('linux', globalWinEnv), 'not Windows', 'S1 linux');
    expectEqual(shortcutMod.skipReason('darwin', globalWinEnv), 'not Windows', 'S1 darwin');
    expectEqual(shortcutMod.skipReason('win32', { APPDATA: 'x' }), 'not a global install',
      'S1 local');
    expectEqual(
      shortcutMod.skipReason('win32', { ...globalWinEnv, AGENTSCOMMANDER_NO_SHORTCUT: '1' }),
      'AGENTSCOMMANDER_NO_SHORTCUT=1',
      'S1 opt-out',
    );
    expectEqual(shortcutMod.skipReason('win32', { npm_config_global: 'true' }),
      'APPDATA is not set', 'S1 appdata');
    expectEqual(shortcutMod.skipReason('win32', globalWinEnv), null, 'S1 global windows');
  });

  test('S2', () => {
    const r = shortcutMod.createStartMenuShortcut('C:\\x\\agentscommander.exe', { platform: 'win32',
      env: { APPDATA: 'x' } });
    expectEqual(r.created, false, 'S2 created');
    assert(!fs.existsSync(path.join(workDir, 'appdata')), 'S2: a skipped call must write nothing');
  });

  if (process.platform !== 'win32') {
    skip('S3', 'real .lnk creation needs a Windows host');
  } else {
    test('S3', () => {
      // Outside bin/: the shell may hold the target open to read its icon, which
      // would make the later bin/ cleanups fail with EPERM.
      // A quote and a dollar sign in the path prove the paths never reach the script text.
      const oddDir = path.join(workDir, 'shortcut-target', "it's $HOME");
      fs.mkdirSync(oddDir, { recursive: true });
      const target = path.join(oddDir, 'agentscommander.exe');
      fs.copyFileSync(process.execPath, target);
      const env = { ...process.env, ...globalWinEnv };
      const first = shortcutMod.createStartMenuShortcut(target, { env });
      expectEqual(first.path, shortcutMod.shortcutPath(env), 'S3 path');
      const again = shortcutMod.createStartMenuShortcut(target, { env });
      expectEqual(again.created, true, 'S3 idempotent re-run');
      const ps = path.join(process.env.SystemRoot || String.raw`C:\Windows`, 'System32',
        'WindowsPowerShell', 'v1.0', 'powershell.exe');
      const read = spawnSync(
        ps,
        ['-NoProfile', '-NonInteractive', '-Command',
          '$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:P); '
            + '$s.TargetPath; $s.IconLocation'],
        { env: { ...process.env, P: first.path }, encoding: 'utf8' },
      );
      expectExit(read, 0, 'S3 read-back');
      const [targetBack, iconBack] = read.stdout.trim().split(/\r?\n/);
      expectEqual(targetBack, target, 'S3 TargetPath');
      expectEqual(iconBack, `${target},0`, 'S3 IconLocation');
      return `wrote ${first.path}`;
    });
  }
}

export function runMacAliasChecks(requireFromWork, desktopContext) {
  const {
    workDir, test, skip, expectEqual, assert, assertThrows, makeBundle,
  } = desktopContext;
  // --- macOS ~/Applications alias (issue #2065) ---
  const aliasMod = requireFromWork(path.join(workDir, 'macos-alias.js'));
  const macHome = path.join(workDir, 'mac-home');
  const globalMacEnv = { npm_config_global: 'true', HOME: macHome };

  test('M1', () => {
    expectEqual(aliasMod.skipReason('win32', globalMacEnv), 'not macOS', 'M1 windows');
    expectEqual(aliasMod.skipReason('linux', globalMacEnv), 'not macOS', 'M1 linux');
    expectEqual(aliasMod.skipReason('darwin', { HOME: '/h' }), 'not a global install', 'M1 local');
    expectEqual(
      aliasMod.skipReason('darwin', { ...globalMacEnv, AGENTSCOMMANDER_NO_SHORTCUT: '1' }),
      'AGENTSCOMMANDER_NO_SHORTCUT=1',
      'M1 opt-out',
    );
    expectEqual(aliasMod.skipReason('darwin', { npm_config_global: 'true' }), 'HOME is not set',
      'M1 home');
    expectEqual(aliasMod.skipReason('darwin', globalMacEnv), null, 'M1 global macos');
    expectEqual(aliasMod.aliasPath({ HOME: '/h' }), path.join('/h', 'Applications',
      'AgentsCommander.app'), 'M1 path');
  });

  test('M2', () => {
    const r = aliasMod.createLaunchpadAlias('/nowhere/Agents Commander.app', { platform: 'linux',
      env: globalMacEnv });
    expectEqual(r.created, false, 'M2 created');
    expectEqual(r.reason, 'not macOS', 'M2 reason');
    assert(!fs.existsSync(macHome), 'M2: a skipped call must write nothing');
  });

  // Windows needs elevation or Developer Mode to create a directory symlink, so
  // probe the real capability instead of assuming it from the platform name.
  const symlinkProbe = (() => {
    const probeTarget = path.join(workDir, 'symlink-probe-target');
    const probeLink = path.join(workDir, 'symlink-probe-link');
    fs.mkdirSync(probeTarget, { recursive: true });
    fs.rmSync(probeLink, { recursive: true, force: true });
    try {
      fs.symlinkSync(probeTarget, probeLink, 'dir');
      fs.unlinkSync(probeLink);
      return null;
    } catch (err) {
      return `directory symlinks are not permitted on this host (${err.code})`;
    }
  })();

  if (symlinkProbe) {
    skip('M3', symlinkProbe);
  } else {
    test('M3', () => {
      // `platform: 'darwin'` drives the real filesystem work on any POSIX host:
      // only Launchpad and Spotlight indexing need a real Mac.
      const bundleParent = path.join(workDir, 'alias-bundle');
      fs.rmSync(bundleParent, { recursive: true, force: true });
      fs.mkdirSync(bundleParent, { recursive: true });
      const { appDir } = makeBundle(bundleParent, { bundleName: 'Agents Commander.app', exeBytes:
        'x' });
      const env = { ...globalMacEnv };
      const opts = { platform: 'darwin', env };

      const first = aliasMod.createLaunchpadAlias(appDir, opts);
      expectEqual(first.created, true, 'M3 created');
      expectEqual(first.path, aliasMod.aliasPath(env), 'M3 path');
      assert(fs.lstatSync(first.path).isSymbolicLink(), 'M3: alias is not a symlink');
      expectEqual(fs.readlinkSync(first.path), appDir, 'M3 target');
      assert(
        fs.existsSync(path.join(first.path, 'Contents', 'Info.plist')),
        'M3: the alias does not resolve into the bundle',
      );

      // A second install replaces the link and leaves exactly one entry.
      const again = aliasMod.createLaunchpadAlias(appDir, opts);
      expectEqual(again.created, true, 'M3 idempotent re-run');
      expectEqual(fs.readdirSync(path.join(macHome, 'Applications')).length, 1, 'M3 one entry');

      // A real app already sitting at that name is never deleted.
      fs.unlinkSync(first.path);
      fs.mkdirSync(first.path, { recursive: true });
      assertThrows(() => aliasMod.createLaunchpadAlias(appDir, opts), ['not a symlink'],
        'M3 foreign app');
      assert(fs.statSync(first.path).isDirectory(), 'M3: the foreign app was removed');
      fs.rmSync(first.path, { recursive: true, force: true });

      assertThrows(
        () => aliasMod.createLaunchpadAlias(path.join(bundleParent, 'Missing.app'), opts),
        ['app bundle not found'],
        'M3 missing bundle',
      );
      assertThrows(
        () => aliasMod.createLaunchpadAlias(bundleParent, opts),
        ['not an app bundle'],
        'M3 not a bundle',
      );
      return `wrote ${first.path}`;
    });
  }
  return symlinkProbe;
}

export function runLinuxEntryChecks(requireFromWork, desktopContext) {
  const {
    workDir, binDir, NPM_DIR, REPO_ROOT, test, expectEqual, assert, assertThrows,
    expectExit, assertIncludes, runInstall, assertNoInstallLeftovers,
  } = desktopContext;
  // --- Linux application menu entry (issue #2066) ---
  const desktopMod = requireFromWork(path.join(workDir, 'linux-desktop-entry.js'));
  const linuxHome = path.join(workDir, 'linux-home');
  const globalLinuxEnv = { npm_config_global: 'true', HOME: linuxHome };
  const entryFile = (dataHome) => path.join(dataHome, 'applications', 'agentscommander.desktop');

  test('L1', () => {
    expectEqual(desktopMod.skipReason('win32', globalLinuxEnv), 'not Linux', 'L1 windows');
    expectEqual(desktopMod.skipReason('darwin', globalLinuxEnv), 'not Linux', 'L1 darwin');
    expectEqual(desktopMod.skipReason('linux', { HOME: '/h' }), 'not a global install', 'L1 local');
    expectEqual(
      desktopMod.skipReason('linux', { ...globalLinuxEnv, AGENTSCOMMANDER_NO_SHORTCUT: '1' }),
      'AGENTSCOMMANDER_NO_SHORTCUT=1',
      'L1 opt-out',
    );
    expectEqual(
      desktopMod.skipReason('linux', { npm_config_global: 'true' }),
      'neither XDG_DATA_HOME nor HOME is set',
      'L1 home',
    );
    expectEqual(desktopMod.skipReason('linux', { npm_config_global: 'true', XDG_DATA_HOME: '/x' }),
      null, 'L1 xdg only');
    expectEqual(desktopMod.skipReason('linux', globalLinuxEnv), null, 'L1 global linux');
    const home = path.join('/h', '.local', 'share');
    expectEqual(desktopMod.entryPath({ HOME: '/h' }), entryFile(home), 'L1 default path');
    expectEqual(desktopMod.entryPath({ HOME: '/h', XDG_DATA_HOME: '' }), entryFile(home),
      'L1 empty XDG_DATA_HOME');
    expectEqual(desktopMod.entryPath({ HOME: '/h', XDG_DATA_HOME: '/data' }), entryFile('/data'),
      'L1 XDG_DATA_HOME');
  });

  test('L2', () => {
    const exe = '/opt/node/lib/node_modules/@mblua/agentscommander/bin/agentscommander';
    const expected = [
      '[Desktop Entry]',
      'Type=Application',
      'Version=1.0',
      'Name=AgentsCommander',
      'Comment=Terminal session manager for AI coding agent teams',
      `Exec="${exe}"`,
      `TryExec=${exe}`,
      'Icon=/p/icon.png',
      'Terminal=false',
      'Categories=Development;Utility;',
      '',
    ].join('\n');
    expectEqual(desktopMod.entryText(exe, '/p/icon.png'), expected, 'L2 contents');
    // Spec quoting: " ` $ \ are backslash-escaped inside the quotes and % is
    // doubled; then every backslash is doubled again for the string value.
    const B = '\\';
    expectEqual(
      desktopMod.quoteExecArg(`/a b/"x"\`$%${B}y`),
      `"/a b/${B}${B}"x${B}${B}"${B}${B}\`${B}${B}$%%${B}${B}${B}${B}y"`,
      'L2 quoting',
    );
    assertThrows(() => desktopMod.entryText('/a\nb', '/p/icon.png'), ['line break'], 'L2 newline');
  });

  test('L3', () => {
    const r = desktopMod.createDesktopEntry('/x/agentscommander', { platform: 'linux', env: { HOME:
      linuxHome } });
    expectEqual(r.created, false, 'L3 created');
    assert(!fs.existsSync(linuxHome), 'L3: a skipped call must write nothing');
  });

  test('L4', () => {
    // `platform: 'linux'` drives the real file write on any host: only the menu
    // itself needs a Linux desktop.
    const target = path.join(workDir, 'linux-target', 'agentscommander');
    const dataHome = path.join(workDir, 'xdg-data');
    const env = { ...globalLinuxEnv, XDG_DATA_HOME: dataHome };
    const opts = { platform: 'linux', env, updateDbCandidates: [path.join(workDir,
      'missing-update-desktop-database')] };
    const first = desktopMod.createDesktopEntry(target, opts);
    expectEqual(first.created, true, 'L4 created');
    expectEqual(first.refreshed, false, 'L4 missing update-desktop-database');
    expectEqual(first.path, entryFile(dataHome), 'L4 path');
    const again = desktopMod.createDesktopEntry(target, opts);
    expectEqual(again.path, first.path, 'L4 idempotent re-run');
    expectEqual(fs.readdirSync(path.dirname(first.path)).length, 1, 'L4 one entry');
    expectEqual(
      fs.readFileSync(first.path, 'utf8'),
      desktopMod.entryText(target, path.join(workDir, 'icon.png')),
      'L4 contents',
    );
    assert(!fs.existsSync(linuxHome), 'L4: XDG_DATA_HOME must win over HOME');
    assertThrows(
      () => desktopMod.createDesktopEntry(target, { ...opts, iconPath: path.join(workDir,
        'no-icon.png') }),
      ['icon not found'],
      'L4 missing icon',
    );
    return `wrote ${first.path}`;
  });

  test('L5', () => {
    const shipped = fs.readFileSync(path.join(NPM_DIR, 'icon.png'));
    const source = fs.readFileSync(path.join(REPO_ROOT, 'src-tauri', 'icons', '128x128.png'));
    assert(shipped.equals(source),
      'L5: npm/icon.png must be a byte copy of src-tauri/icons/128x128.png');
  });

  // --- the linux branch of install.js, end to end (issue #2066) ---
  const linuxFixture = 'linux-raw';
  fs.writeFileSync(path.join(workDir, linuxFixture), 'fixture linux executable placeholder\n');
  const linuxJobs = [
    { id: 'I5', env: { npm_config_global: 'true' }, expectEntry: true },
    { id: 'I6', env: { npm_config_global: 'true', AGENTSCOMMANDER_NO_SHORTCUT: '1' }, expectEntry:
      false },
    { id: 'I7', env: {}, expectEntry: false },
  ];
  for (const job of linuxJobs) {
    test(job.id, () => {
      fs.rmSync(binDir, { recursive: true, force: true });
      const dataHome = path.join(workDir, `xdg-${job.id}`);
      fs.rmSync(dataHome, { recursive: true, force: true });
      const r = runInstall(linuxFixture, { dir: null }, {
        I2016_PLATFORM: 'linux',
        I2016_ASSET: 'agentscommander-linux-x86_64',
        XDG_DATA_HOME: dataHome,
        ...job.env,
      });
      expectExit(r, 0, `${job.id} install`);
      assertIncludes(r.stdout, 'Installation completed successfully.', `${job.id} stdout`);
      const exe = path.join(binDir, 'agentscommander');
      assert(fs.existsSync(exe), `${job.id}: expected installed executable at ${exe}`);
      assertNoInstallLeftovers(job.id);
      if (!job.expectEntry) {
        assert(!fs.existsSync(dataHome), `${job.id}: must create no menu entry`);
        return 'created nothing';
      }
      const entry = entryFile(dataHome);
      assertIncludes(r.stdout, `Application menu entry created: ${entry}`, `${job.id} stdout`);
      expectEqual(
        fs.readFileSync(entry, 'utf8'),
        desktopMod.entryText(exe, path.join(workDir, 'icon.png')),
        `${job.id} contents`,
      );
      return `entry ${entry}`;
    });
  }
}

