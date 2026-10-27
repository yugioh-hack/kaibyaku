export type Status = "確定" | "要確認" | "カードなし" | "エラー";

export interface DataSummary {
  cards: number;
  hashes: number;
  names: number;
  english_names: number;
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
  asked_english_names?: boolean;
}

/** 旧アプリのデータの取り込み結果（取り込みをしたときだけ中身がある） */
export interface Migration {
  copied?: string[];
  corrupt?: string[];
  error?: string;
}

export interface AppState {
  migration?: Migration | null;
  version: string;
  user_data: string;
  default_output: string;
  config: Config;
  data: DataSummary;
}

/** 処理を始めたときの出力先と設定。再処理と古いコピーの掃除には、画面の現在値ではなくこれを使う。 */
export interface ProcessArgs {
  output: string;
  save_format: string;
  rounded: boolean;
  recursive: boolean;
  skip_duplicate: boolean;
  save_review: boolean;
  save_not_card: boolean;
  english: string;
  split: string;
  resolution: string;
}

/** ログ（log.csv）を書けなかったこと。画像の処理結果とは別に知らせる。 */
export interface CsvError {
  path: string;
  message: string;
}

export interface IdentifyResult {
  source: string;
  status: Status;
  card_id: number | string | null;
  name: string | null;
  distance: number;
  margin: number;
  saved_path: string | null;
  /** この結果が実際に作ったファイルすべて（both 形式なら2つ） */
  saved_paths?: string[];
  /** この結果を作った処理の条件（画面側で付ける） */
  args?: ProcessArgs;
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
  csv_error?: CsvError | null;
}

export interface Reprocessed {
  result: IdentifyResult;
  csv_error?: CsvError | null;
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
