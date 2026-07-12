# AI Proxy (Hot Memory & Context Management Layer)

本プロジェクトは、24時間稼働の自律型パーソナルAIナレッジシステムにおける「中間層（プロキシサーバー）」の実装である。各センサー（スクリーンショットOCR、Memos、会話ログ等）から送られてくるリアルタイムのインプットを高速に裁き、インメモリデータストア（Redis）を用いた「Hot Memory（短期記憶スロット）」の管理およびバックグラウンドでの長期記憶昇華判定を行う。

---

## 1. システムアーキテクチャ & ディレクトリ構成

保守性と拡張性を担保するため、以下の構成に基づき、機能ごとにファイルを分割して実装すること。

```text
ai-proxy/
├── Cargo.toml               # 依存関係定義
└── src/
    ├── main.rs              # エントリポイント（サーバー起動 & ワーカーの並行実行）
    ├── config.rs            # 環境変数およびRedis接続設定
    ├── models.rs            # データ構造・型（Struct）の定義
    ├── api/                 # APIルーティングおよびハンドラ層
    │   ├── mod.rs
    │   └── logs.rs          # POST /api/logs の受付ロジック
    ├── services/            # ビジネスロジック（Redis操作・スロット管理）
    │   ├── mod.rs
    │   └── slot_manager.rs  # スロットの判定・タイムライン追記・新規生成
    └── workers/             # バックグラウンド非同期タスク
        ├── mod.rs
        └── timeout_monitor.rs # 15分非アクティブスロットの監視・回収・削除ループ

```

---

## 2. 技術スタック & 依存ライブラリ (`Cargo.toml`)

以下のライブラリを使用し、非同期I/O処理および型安全性を徹底すること。

* **非同期ランタイム**: `tokio` (features = ["full"])
* **Webフレームワーク**: `axum` (features = ["json"])
* **Redisクライアント**: `redis` (features = ["tokio-comp", "aio"]) -> 非同期マルチプレクス接続（`aio::MultiplexedConnection`）を使用すること。
* **シリアライズ/デシリアライズ**: `serde` (features = ["derive"]), `serde_json`
* **日付・時間・ID**: `chrono` (UTC/ローカルタイムスタンプ管理用), `uuid` (features = ["v4"])

---

## 3. Redis データ構造設計 (Schema Design)

Redis（`localhost:6379` で稼働）内では、以下の3つのキー構造を用いてマルチスロット形式でHot Memoryを運用する。

### 3.1 スロットメタデータ (Hash)

各作業コンテクスト（トピック）の基本情報と最終更新時刻を保持する。

* **Key**: `slot:{uuid}:meta`
* **Fields**:
* `topic`: 文字列（例: `"英文読解 ソクモン 開発"`, `"大学の化学の課題提出"`）
* `focused_file`: 文字列（例: `"src/components/Button.tsx"`, `"None"`)
* `last_updated`: i64 (Unixタイムスタンプ。ログが追加されるたびに現在時刻へ更新)



### 3.2 タイムライン (List)

スロット内部で発生した個々のイベントログを時系列順に格納する（末尾追加）。

* **Key**: `slot:{uuid}:timeline`
* **Value (JSON文字列)**:
```json
{
  "timestamp": "2026-07-12T17:36:00Z",
  "source": "ocr" | "memos" | "voice",
  "content": "自由記述テキスト、または解析されたメタデータ"
}

```



### 3.3 アクティブスロット追跡 (Set)

現在Hot Memory上で稼働している全スロットのUUIDを一覧保持する。ワーカーの監視対象リストとなる。

* **Key**: `active_slots`
* **Value**: `["uuid_1", "uuid_2", ...]`

---

## 4. 詳細仕様 & アルゴリズム実装指示

### 4.1 Ingestion API (`POST /api/logs`)

* **エンドポイント**: `/api/logs`
* **リクエストボディ (JSON)**:
```rust
struct IngestLogRequest {
    source: String,       // "ocr", "memos" 等
    topic_hint: String,   // 検出されたプロジェクト名やウィンドウタイトル
    focused_file: Option<String>,
    content: String,
}

```


* **挙動**:
1. リクエストを受信したら、即座に `services::slot_manager` へ処理を委譲する。
2. メモリ上での振り分けおよびRedisへの書き込みが完了したら、速やかに `200 OK` (JSON: `{"status": "success", "slot_id": "..."}`) を返却し、クライアント側の接続を長時間拘束しないこと。



### 4.2 スロット判定・振り分けロジック (`slot_manager.rs`)

新しいログが届いた際、どのスロットに格納するかを以下のステップで決定論的に判定する。

1. **既存スロットの走査**:
Redisの `active_slots` に登録されている全UUIDについて、`slot:{uuid}:meta` の `topic` フィールドをロードする。
2. **マッチング判定**:
届いたログの `topic_hint` が、既存スロットの `topic` と完全一致、あるいは部分一致（インテリジェントな前方一致・包含ルール）するかを検証する。
3. **分岐処理**:
* **一致するスロットが存在する場合**:
* 該当スロットの `slot:{uuid}:timeline` に対し、`RPUSH` で新規ログをシリアライズして追加。
* `slot:{uuid}:meta` の `last_updated` を現在のUnixタイムスタンプに上書き（更新）。
* `focused_file` に変更があればそれも上書き。


* **一致するスロットが存在しない、またはコンテクストが新規の場合**:
* 新しい `uuid::Uuid::new_v4()` を発行。
* `active_slots` (Set) にそのUUIDを追加。
* `slot:{uuid}:meta` (Hash) を新規に作成（`topic`, `focused_file`, `last_updated` をセット）。
* `slot:{uuid}:timeline` (List) に最初のログを追加。





### 4.3 タイムアウト監視ワーカー (`timeout_monitor.rs`)

APIサーバーの起動と同時に、`tokio::spawn` を用いてバックグラウンドで完全に独立した無限ループ処理（周期: 60秒）を稼働させる。

* **アルゴリズム**:
1. `active_slots` から現在のアクティブなUUID一覧を取得する。
2. 各UUIDについて、`slot:{uuid}:meta` 内の `last_updated` の値をチェックする。
3. **【条件: 現在時刻 - last_updated > 900秒 (15分)】** を満たすスロットを検出した場合、そのスロットは「ユーザーが作業を終了、または別のタスクに移った（非アクティブ）」と判定する。
4. **フラッシュ処理**:
* 該当スロットのメタデータおよびタイムライン（Listの全要素）をRedisから全て読み出す。
* 読み出したデータブロックを、将来的な長期記憶（Vector DB / Graph DB）永続化レイヤーへと引き渡すためのログ出力、または内部関数（プレースホルダー `save_to_long_term_storage(slot_data)`）へフォワードする。
* 処理完了後、Redis上の `slot:{uuid}:meta`, `slot:{uuid}:timeline` を `DEL` し、`active_slots` から該当UUIDを `SREM`（削除）する。





### 4.4 コンテクスト・シフト判定 (即時フラッシュロジック)

`slot_manager.rs` において、特定の既存スロット（例: スロットA）の `focused_file` や `topic` が開かれている状態のまま、全く異なるコンテクスト（例: スロットB）のログが連続して急激に（例: 直近3分間に3回以上）流入したことを検知した場合、「15分の自然タイムアウト」を待たずに、スロットAのタイマーを満了（あるいは即座にフラッシュ関数を呼び出しクローズ）させるショートカットパスを設けること。これにより、ランダムなタスク切り替え時に古いスロットがダラダラとHot Memoryに残り続けることを防ぐ。

---

## 5. 実装時の注意点

* **エラーハンドリング**: Redisへの接続瞬断やパース失敗が起きても、パニック（`panic!`）してプロセス全体を巻き込んでクラッシュさせないこと。エラーは適切にログ（`tracing` または `log` クレート）に吐き出し、サーバーの稼働を維持すること。
* **共有状態 (Shared State)**: Axumのハンドラ間でRedisのマルチプレクス接続プールや設定情報を共有するため、`axum::Extension` または `State` 抽象化を正しく適用すること。
