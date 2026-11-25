import type { AppSettings } from "./types";

/** #5228 - flags stored by the backend (P01); not part of AppSettings to keep types.ts small. */
export type TutorialSettings = {
  tutorialHintDismissed?: boolean;
  teamTutorialCompleted?: boolean;
};

export type TutorialFlags = Required<TutorialSettings>;

export function readTutorialFlags(settings: AppSettings): TutorialFlags {
  const s = settings as AppSettings & TutorialSettings;
  return {
    tutorialHintDismissed: s.tutorialHintDismissed === true,
    teamTutorialCompleted: s.teamTutorialCompleted === true,
  };
}
