# AI Proxy (Hot Memory & Context Management Layer)

本プロジェクトは、24時間稼働の自律型パーソナルAIナレッジシステムにおける「中間層（プロキシサーバー）」の実装です。各センサー（スクリーンショットOCR、Memos、会話ログ等）から送られてくるリアルタイムのインプットを高速に裁き、インメモリデータストア（Redis）を用いた「Hot Memory（短期記憶スロット）」の管理およびバックグラウンドでの長期記憶昇華判定を行います。

---

## 1. システムアーキテクチャ & ディレクトリ構成

保守性と拡張性を担保するため、以下の構成に基づき、機能ごとにファイルを分割して実装しています。

```text
ai-proxy/
├── Cargo.toml               # 依存関係定義
└── src/
    ├── main.rs              # エントリポイント（サーバー起動 & ワーカーの並行実行）
    ├── config.rs            # 環境変数（Redis, AIサーバー）設定
    ├── models.rs            # データ構造・型（Struct）の定義
    ├── api/                 # APIルーティングおよびハンドラ層
    │   ├── mod.rs
    │   └── logs.rs          # POST /api/logs の受付ロジック
    ├── services/            # ビジネスロジック
    │   ├── mod.rs
    │   ├── ai_processor.rs  # OpenAI Compatible API による topic 抽出 & 要約
    │   └── slot_manager.rs  # スロットの判定・タイムライン追記・新規生成
    └── workers/             # バックグラウンド非同期タスク
        ├── mod.rs
        └── timeout_monitor.rs # 15分非アクティブスロットの監視・回収・削除ループ
```

---

## 2. 技術スタック & 依存ライブラリ (`Cargo.toml`)

| ライブラリ | 用途 |
|------------|------|
| `tokio` (features = ["full"]) | 非同期ランタイム |
| `axum` (features = ["json"]) | Webフレームワーク |
| `redis` (features = ["tokio-comp", "aio"]) | 非同期マルチプレクス接続 |
| `serde`, `serde_json` (features = ["derive"]) | シリアライズ/デシリアライズ |
| `chrono` | 日付・時間管理 |
| `uuid` (features = ["v4"]) | UUID 生成 |
| `reqwest` (features = ["json", "rustls-tls"]) | OpenAI Compatible API への HTTP リクエスト |

---

## 3. 環境変数

| 変数名 | 必須 | デフォルト値 | 説明 |
|--------|------|-------------|------|
| `PORT` | 任意 | `3000` | サーバーのリッスンポート番号 |
| `REDIS_URL` | 任意 | `redis://127.0.0.1:6379` | Redis 接続 URL |
| `AI_BASE_URL` | 任意 | `http://localhost:8080/v1` | OpenAI Compatible API のベース URL |
| `AI_API_KEY` | 任意 | (なし) | API キー（Bearer トークンとして送信） |
| `AI_MODEL` | 任意 | `gpt-4o-mini` | 使用するモデル名 |

---

## 4. Redis データ構造設計

### 4.1 スロットメタデータ (Hash)

* **Key**: `slot:{uuid}:meta`
* **Fields**
  * `topic` – AI が抽出したコンテキストトピック文字列
  * `focused_file` – 現在フォーカス中のファイル（無い場合は `"None"`）
  * `last_updated` – Unix タイムスタンプ (i64)

### 4.2 タイムライン (List)

* **Key**: `slot:{uuid}:timeline`
* **Value** – JSON 文字列
```json
{
  "timestamp": 1750822560,
  "source": "ocr",
  "content": "AI によって要約されたテキスト"
}
```

### 4.3 アクティブスロット追跡 (Set)

* **Key**: `active_slots`
* **Value** – UUID の集合 (`["uuid_1","uuid_2",...]`)

---

## 5. API 使用方法

### 5.1 Ingestion API (`POST /api/logs`)

#### エンドポイント
```
POST http://<host>:<port>/api/logs
Content-Type: application/json
```

#### リクエストボディ (JSON)
```json
{
  "source": "ocr",
  "content": "スクリーンショットから抽出した生テキスト..."
}
```

| フィールド | 型 | 必須 | 説明 |
|-----------|------|------|------|
| `source` | string | はい | データソース（`"ocr"`, `"memos"`, `"voice"` 等） |
| `content` | string | はい | 生の未処理データ |

#### 処理フロー
1. リクエスト受信後、`ai_processor` が OpenAI Compatible API を呼び出し、`content` から **topic**（トピックラベル）と **summary**（要約）を抽出
2. 抽出された `topic` をもとに `slot_manager` が既存スロットへの振り分け or 新規スロット作成を実行
3. タイムラインには要約された `summary` が格納される

#### 成功レスポンス
```json
{
  "status": "success",
  "slot_id": "c0a1b2d3-4e5f-6789-abcd-ef0123456789"
}
```
ステータスコードは **200 OK**。

#### エラーレスポンス
```json
{
  "status": "error",
  "message": "AI processing failed: ..."
}
```
ステータスコードは **500 Internal Server Error**。

#### curl の例
```bash
curl -X POST http://localhost:3000/api/logs \
  -H "Content-Type: application/json" \
  -d '{
        "source": "ocr",
        "content": "スクリーンショットから抽出したテキスト: 英文読解の課題、ソクモン開発の続き"
      }'
```

---

## 6. 主なロジック概要

### 6.1 `ai_processor.rs` – AI による topic 抽出 & 要約
* OpenAI Compatible API (`/v1/chat/completions`) を呼び出し
* システムプロンプトで「topic（1-5語のラベル）」と「summary（1-3文の要約）」を JSON 形式で返すよう指示
* `response_format: json_object` で構造化出力を強制
* エラー時は `panic!` せず、エラーメッセージを呼び出し元に返す

### 6.2 `slot_manager.rs` – スロット判定・振り分け
* **既存スロット走査**: `active_slots` の UUID を列挙し、各 `slot:{uuid}:meta.topic` と AI が抽出した `topic` を比較
* **マッチング**: 完全一致または部分包含で判断
* **一致時**: `timeline` へ RPUSH（要約を格納）、`last_updated` 更新
* **不一致時**: 新規 UUID を生成 → `active_slots` に追加 → `meta` と `timeline` を新規作成

### 6.3 `timeout_monitor.rs` – 非アクティブスロット回収
1. 60 秒ごとに `active_slots` 全体を取得
2. 各スロットの `last_updated` が現在時刻から 900 秒 (>15 分) 超えていれば **フラッシュ**
3. フラッシュ処理: Redis 上の `meta` と `timeline` を削除し、`active_slots` から UUID を除外

### 6.4 コンテキストシフト判定 (即時フラッシュ)
* 同一スロットで連続的にログが流入した場合（例: 3 分間に 3 回以上）
* `slot_manager` が即座に他の非アクティブスロットをフラッシュし、リソースの無駄な保持を防止

---

## 7. 実装時の注意点

* **エラーハンドリング**: Redis 接続失敗、AI API エラー、JSON パースエラーは `panic!` せず、`tracing` に記録してサーバー稼働を維持すること。
* **共有状態**: `axum::Extension` を用いて Redis 接続プールと設定情報をハンドラ間で安全に共有する。
* **テスト**: `tests/integration_test.rs` でエンドポイントの正常系シナリオを検証。テスト実行には Redis と AI サーバーの起動が必要。

---

## 8. 今後の拡張案

* 長期記憶層としてベクトルデータベース（例: Pinecone, Qdrant）への自動バックアップ
* WebSocket を用いたリアルタイム通知機構の追加
* 認証・認可レイヤー（API キーや OAuth2）によるセキュリティ強化
* AI 処理のタイムアウト・リトライ機構