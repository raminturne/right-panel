# Plugin API v1
The sandbox exposes `window.RightPanel`: `plugin.id`, `plugin.version`, `storage.get/set/remove/clear`, `clipboard.read/write`, `openUrl`, and `ui.close/showToast`. All return Promises. Storage requires `storage`; clipboard operations require their matching permission; URLs require `system.openUrl` and are restricted to HTTP(S).
