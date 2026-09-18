AgentsCommander {{VERSION}}
Portable build for Windows x86_64

WHAT THIS IS

  A standalone AgentsCommander. There is no installer and nothing is written
  to the Windows registry. Unzip it anywhere you can write, including a USB
  drive or a network share, and run agentscommander.exe.


WHERE YOUR DATA LIVES

  agentscommander.exe keeps its state in your user profile, not next to the
  executable:

      %USERPROFILE%\.agentscommander\

  That folder holds settings.json, sessions.json, the web token, logs, and
  every other piece of per-instance state. Moving the executable does not
  move the instance; the state stays in your profile.

  A renamed copy with an underscore suffix keeps its state next to the
  executable instead, in a folder named after it, and only there:

      agentscommander_teamA.exe   ->   .agentscommander_teamA\

  AGENTSCOMMANDER_CONFIG_DIR overrides both cases: a nonblank value selects
  that directory instead. Prefer an absolute path.

  Projects you register are stored as absolute paths. The profile folder has
  no instance base, so a project registered with the default exe keeps only
  its absolute path. A suffixed copy uses the folder holding the executable
  as its instance base, so a project moved together with the copy is found
  again at its new location.


RUNNING MORE THAN ONE INSTANCE

  Rename the executable with an underscore suffix:

      agentscommander_teamA.exe   ->   .agentscommander_teamA\

  A renamed copy is a fully independent instance: its own settings, its own
  sessions, its own ports, and its own single-instance lock. The suffix shows
  as a badge in the title bar.

  The adjacent folder must be writable. AgentsCommander probes it before
  starting and refuses to launch when it cannot write there, telling you to
  move the executable to a writable folder or set AGENTSCOMMANDER_CONFIG_DIR
  to a writable directory. A suffixed copy never falls back to the user
  profile.

  Two copies with the SAME file name are the same instance identity even in
  different folders, so the second one will not open a second window. Rename
  it if you want both running at once.


REQUIREMENTS

  Windows 10 version 1809 or newer, x86_64.

  The Microsoft Edge WebView2 runtime must be installed. It is already
  present on Windows 11 and on up-to-date Windows 10. If the app does not
  open a window, install the Evergreen runtime from:

      https://developer.microsoft.com/microsoft-edge/webview2/


UPDATING

  Close the app, replace agentscommander.exe with the newer one, and start
  it again.

  For the default exe your settings, projects, and sessions are preserved:
  they live in %USERPROFILE%\.agentscommander\, which the download does not
  change.

  For a suffixed copy, keep its .agentscommander_<suffix>\ folder next to
  the executable. Replacing only the exe preserves the copy's settings.


UNINSTALLING

  Deleting the unzipped folder does not remove everything. Delete the state
  yourself:

      default exe : %USERPROFILE%\.agentscommander\
      suffixed    : .agentscommander_<suffix>\ next to the executable
      override    : the directory named by AGENTSCOMMANDER_CONFIG_DIR

  These folders hold your settings and tokens, so delete them only when you
  no longer need them.


VERIFYING THIS DOWNLOAD

  Windows code signing is not yet in place, so SmartScreen will warn on first
  launch. Verify the download against SHASUMS256.txt on the release page
  instead:

      Get-FileHash .\agentscommander.exe -Algorithm SHA256

  Compare the result with the entry for this zip in SHASUMS256.txt. Signing
  is tracked in https://github.com/mblua/AgentsCommander/issues/717


MORE

  Directory layout   : https://github.com/mblua/AgentsCommander/blob/main/docs/reference/directory-layout.md
  Portable instances : https://github.com/mblua/AgentsCommander/blob/main/docs/features/portable-instances.md
  Quickstart         : https://github.com/mblua/AgentsCommander/blob/main/docs/quickstart.md
  Privacy            : https://github.com/mblua/AgentsCommander/blob/main/PRIVACY.md
  Issues             : https://github.com/mblua/AgentsCommander/issues

  Licensed under the MIT License. See LICENSE and THIRD_PARTY_NOTICES.md.
