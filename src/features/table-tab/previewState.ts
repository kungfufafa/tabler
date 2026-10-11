export interface PreviewState {
  open: boolean;
  statements: string[];
  loading: boolean;
  applying: boolean;
  error: string | null;
  failedIndex: number | null;
}

export const PREVIEW_CLOSED: PreviewState = {
  open: false,
  statements: [],
  loading: false,
  applying: false,
  error: null,
  failedIndex: null,
};
