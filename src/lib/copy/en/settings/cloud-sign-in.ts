// The "sign in to use embral cloud" dialog. The component lives in the shared
// settings tree (not the cloud seam) and is only mounted in cloud builds, so
// its copy lives here in the shared catalog — a shared-tree component must not
// import from src/lib/cloud/ (docs/cloud-seam.md).

export const cloudSignIn = {
  title: 'Sign in to use embral cloud',
  description:
    "Cloud transcription and summaries run on embral's servers; sign in to use them",
  notNow: 'Not now',
  goToAccount: 'Go to Account'
};
