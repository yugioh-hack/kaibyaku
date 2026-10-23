export type Status = "確定" | "要確認" | "カードなし" | "エラー";

export interface DataSummary {
  cards: number;
  hashes: number;
  names: number;
  names_fetched: boolean;
}

export interface Config {
  output_dir?: string;
  resolution?: string;
  save_format?: string;
  english?: string;
  split?: string;
  rounded?: boolean;
  recursive?: boolean;
  skip_duplicate?: boolean;
  save_review?: boolean;
  save_not_card?: boolean;
  asked_names?: boolean;
}

export interface AppState {
  version: string;
  user_data: string;
  default_output: string;
  config: Config;
  data: DataSummary;
}

export interface IdentifyResult {
  source: string;
  status: Status;
  card_id: number | string | null;
  name: string | null;
  distance: number;
  margin: number;
  saved_path: string | null;
  /** 重複でスキップしたときの、同じイラストの既存ファイル */
  existing_path?: string;
  /** 元画像の大きさ [幅, 高さ] と、判定に使ったカードの枠 [左, 上, 右, 下] */
  size?: [number, number];
  box?: [number, number, number, number] | null;
  ranked?: Array<[number, string]>;
  note: string;
  english: boolean;
}

export interface IdentifyProgress {
  done: number;
  total: number;
  result: IdentifyResult;
}

export interface IdentifyDone {
  confirmed?: number;
  review?: number;
  not_card?: number;
  error?: number | string;
  skipped?: number;
  seconds?: number;
  total?: number;
  cancelled?: boolean;
}

export interface UpdateDone {
  message?: string;
  error?: string;
}

export interface CardRef {
  cid: string;
  name: string;
}

export interface Candidate extends CardRef {
  distance?: number;
}

export interface LearnResult {
  message: string;
  reprocess: boolean;
}
