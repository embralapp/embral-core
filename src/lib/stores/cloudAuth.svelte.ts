import { invoke } from "@tauri-apps/api/core";
import { CLOUD_ENABLED } from "$lib/cloud";

// Tracks whether this device holds a cloud session, so the settings selectors
// can refuse to switch a provider or engine to "embral cloud" while signed
// out (and point the user at the Account page instead). Lives outside the
// cloud-only tree (it only invokes the `cloud_account_status` command and
// never touches src/lib/cloud/), so shared settings code may import it
// directly.
//
// `signed_in` is true whenever a local session token exists, even when the
// server is unreachable (`offline`). That is the right gate: the requirement
// only blocks switching to cloud with no account at all; a dropped
// connection afterwards is the normal runtime failure the app already handles.

let _signedIn = $state(false);
let _promptOpen = $state(false);

async function refresh(): Promise<void> {
  if (!CLOUD_ENABLED) return;
  try {
    const status = await invoke<{ signed_in: boolean }>("cloud_account_status");
    _signedIn = status.signed_in;
  } catch {
    // Command rejected unexpectedly: keep the last known answer rather than
    // flipping a signed-in user to blocked on a transient error.
  }
}

export const cloudAuth = {
  get signedIn() {
    return _signedIn;
  },
  get promptOpen() {
    return _promptOpen;
  },
  set promptOpen(v: boolean) {
    _promptOpen = v;
  },
  refresh,

  /**
   * Gate a switch to an embral cloud option. Returns true when it may proceed;
   * when it can't (no local session), opens the sign-in prompt and returns
   * false so the caller leaves the current value untouched.
   */
  requireSignedIn(): boolean {
    if (_signedIn) return true;
    _promptOpen = true;
    return false;
  },
};
