# 🧠 長期記憶編纂・管理エージェント 仕様書 (Long-Term Memory Compiler & Janitor Specification)

本ドキュメントは、短期記憶（Hot Memory / Redis）からフラッシュされた断片的な時系列ログを、一般化された長期記憶（Qdrant + Neo4j）へ構造化して書き込む「編纂エンジン」と、蓄積された記憶を定期的にクレンジング・最適化する「管理（クローラー）エンジン」の統合仕様である。

---

## 1. 汎用データスキーマ (Generalized Schema)

特定ドメインに依存しない5つの「汎用ノード（Labels）」と4つの「汎用リレーション（Relations）」で世界を表現する。

### 1.1 Neo4j (Graph DB) 汎用オントロジー

* **`(:User)`**: ユーザー自身（固定で `{name: "GoshiEta"}`）
* **`(:Context)`**: 行動の文脈、帰属先（例: `{"name": "英文読解 ソクモン"}`、`{"name": "大学化学"}`、`{"name": "Minecraft 鉄道敷設"}`）
* **`(:Item)`**: 具体的な概念、技術、ツール、法則（例: `{"name": "React"}`、`{"name": "酸化還元反応"}`、`{"name": "加速レール"}`）
* **`(:Artifact)`**: 生成された成果物、特定の数値、座標、ファイル（例: `{"name": "src/main.rs"}`、`{"name": "反応式ノート"}`、`{"name": "X:125, Z:-340"}`）
* **`(:MemoryChunk)`**: 昇華された出来事の最小単位。
* プロパティ: `{slot_id: "string", timestamp: i64, summary: "string", domain: "string"}`



### 1.2 Qdrant (Vector DB) 汎用ペイロード

* **Collection**: `user_memories` (Cosine類似度 / 1536次元など)
* **Payload**:
```json
{
  "summary": "LLMが生成した高密度な要約テキスト",
  "timestamp": 1781848200,
  "context_name": "Minecraft 鉄道敷設",
  "domain": "development" | "study" | "game" | "life" | "other",
  "slot_id": "uuid_v4_string"
}

```



---

## 2. 長期記憶への編纂（書き込み）パイプライン

Hot Memory のスロット（15分非アクティブ判定、またはコンテクスト断絶による即時満了）がフラッシュされた瞬間に駆動する。

### Step 1: 記憶編纂LLMプロンプトの実行

フラッシュされた生ログ（JSONの配列）を以下のシステムプロンプトに投入し、完全な構造化JSONとして抽出する。

```markdown
# 役割
あなたは自律型パーソナルAIの「記憶編纂システム」です。ユーザーの断片的な行動ログ（時系列テキスト、スクショOCR、日常メモ）を分析し、未来のAIエージェントが瞬時に文脈を理解できるよう、「客観的な要約（Summary）」と「知識グラフ（トリプル）」へ厳密に構造化・圧縮しなさい。

# 要約（Summary）の生成ルール
以下の4つの要素を必ず含め、客観的な3人称（「ユーザーは〜」）で、1文〜最大3文の簡潔なMarkdownテキストとして出力しなさい。
1. 【文脈/目的】ユーザーが何を目的として、どのコンテクスト（開発、勉強、ゲーム、生活手続き等）で動いていたか。
2. 【行動/事象】ログの中で発生した決定的な出来事（エラー、特定の計算、座標のメモ、文章の作成など）。
3. 【知見/データ】ログから得られた具体的な数値、座標、ソースコードの修正点、数式の作り方などの「コアデータ」。
4. 【状態】最終的にそのタスクがどうなったか（解決した、中断した、課題が残っているなど）。
※「〜と思われる」「〜かもしれない」といった曖昧な推測は一切禁止。ログにある事実のみを記述すること。

# 知識グラフ（トリプル）の抽出ルール
入力ログから重要な実体（Entity）を抽出し、以下の定義に厳密に沿ってJSON配列として出力しなさい。

## エンティティ・タイプ（Labels）
- User: ユーザー自身（固定で "GoshiEta" とする）
- Context: プロジェクト名、科目名、ゲームの目的、生活のタスク名など（例: "英文読解 ソクモン", "大学化学", "Minecraft"）
- Item: 使用したツール、プログラミング言語、化学の反応名、ゲーム内のアイテム名など（例: "Next.js", "酸化還元反応", "加速レール"）
- Artifact: 具体的な成果物、ファイルパス、具体的な数値データ、座標など（例: "src/main.rs", "X:125, Z:-340", "問い合わせメール文面"）

## リレーション・タイプ（Relations）
- "ENGAGED_IN": User が Context に取り組んでいるとき
- "TOUCHED": 今回の出来事が Item に触れた、あるいは使用したとき
- "PRODUCED": 今回の出来事によって Artifact が生成・変更・確定されたとき

# 出力フォーマット
出力は必ず以下のJSON形式のみとし、余計な挨拶や解説のテキストは一切含めてはならない。

```json
{
  "summary": "（生成ルールに沿った要約）",
  "domain": "development | study | game | life | other",
  "triples": [
    {
      "source": "実体Aの名称",
      "source_type": "User | Context | Item | Artifact",
      "relation": "ENGAGED_IN | TOUCHED | PRODUCED",
      "target": "実体Bの名称",
      "target_type": "User | Context | Item | Artifact"
    }
  ]
}

```


### Step 2: 2つのデータベースへの同時コミット
1.  **Qdrant への Upsert**: `summary` を Embedding 化し、メタデータ（`domain`, `context_name`, `slot_id`）と共に `user_memories` コレクションへ書き込む。
2.  **Neo4j への Cypher 実行**:
    LLMから受け取った `triples` をループし、以下のCypherクエリでグラフを動的に編み上げる（`MERGE` を使うことで、既存ノードが存在すればプロパティの上書きや接続のみを行い、重複を作らない）。

```cypher
// 1. メモリチャンクの生成
MERGE (m:MemoryChunk {slot_id: $slot_id})
SET m.timestamp = $timestamp, m.summary = $summary, m.domain = $domain

// 2. トリプルの動的マージ（各要素のラベルを動的に反映させて実行）
// 例: Source=Context, Target=Item, Relation=TOUCHED の場合
MERGE (s:Context {name: $source_name})
MERGE (t:Item {name: $target_name})
MERGE (s)-[:TOUCHED]->(t)
MERGE (m)-[:BELONGS_TO]->(s)

```

---

## 3. 長期記憶の定期管理（Janitor）機能

週次（クローンジョブ等）で自動実行され、記憶を「より扱いやすく、検索ノイズの少ない状態」へ自律的にメンテナンスする仕組み。

### 3.1 近似ノード・類似エンティティの自動統合（マージ）

ユーザーのタイポや、LLMの表記揺れ（例: `Next.js` と `nextjs`、`酸化還元反応` と `酸化還元`）によって分断されたグラフを修復する。

* **アルゴリズム**:
1. Neo4j から `Context`, `Item`, `Artifact` の各ラベルごとに、`name` プロパティのリストを全件ロードする。
2. 文字列の類似度算出（レーベンシュタイン距離）または軽量Embeddingのコサイン類似度を用いて、「類似度 90% 以上」のノードペアを検出する。
3. 検出されたペア（例: ノードA = `Next.js`, ノードB = `nextjs`）を、以下のCypherクエリを用いて**ノードA側へ統合**する。既存のリレーションシップはすべてノードAに継承（コピー）させ、古いノードBを削除する。



```cypher
MATCH (target:Item {name: "Next.js"}), (source:Item {name: "nextjs"})
// source に繋がっている全てのリレーションを target に付け替える
MATCH (source)-[r]->(x)
FOREACH (ignore IN CASE WHEN NOT (target)-[r]->(x) THEN [1] ELSE [] END |
  // 存在しないリレーションなら新しく作成
  CALL apoc.create.relationship(target, type(r), properties(r), x) YIELD rel
)
// 逆方向のリレーションも同様に処理
MATCH (y)-[r]->(source)
FOREACH (ignore IN CASE WHEN NOT (y)-[r]->(target) THEN [1] ELSE [] END |
  CALL apoc.create.relationship(y, type(r), properties(r), target) YIELD rel
)
// 古いノードとリレーションの削除
DETACH DELETE source

```

### 3.2 メモリ・コンパクション（エピソード記憶の風化と結晶化）

数ヶ月前の古い、かつ細かすぎる `MemoryChunk`（例: 「Button.tsxのタイポを直した」「Button.tsxにログを追加した」など）は、検索時のノイズになるため、1つの「大きな結晶（マクロ記憶）」に統合して圧縮する。

* **条件**: `timestamp` が30日以上前であり、かつ同じ `Context`（プロジェクトや科目）に属する複数の `MemoryChunk`。
* **処理フロー**:
1. 条件に該当する細かな `MemoryChunk` 群の `summary` とグラフ構造をすべて読み出す。
2. LLMに渡し、「この1ヶ月間の同一文脈における活動を、1つの洗練された『月間エピソード要約』にコンパクション（圧縮）せよ」と指示する。
3. **置換**: Qdrant から古い細かなベクトル群を削除し、新しい「月間エピソード要約」のベクトルを1件だけ登録する。Neo4j 側も、古い複数の `MemoryChunk` ノードを削除し、新しい `(:MonthlyMemoryChunk)` ノード1つに集約・接続し直す。



### 3.3 グラフとベクトルの整合性監査（Orphan Cleanup）

システムエラーやクラッシュによって、片方のデータベースだけにデータが残る「孤立データ」をクレンジングする。

* **チェック1**: Qdrant の全 `slot_id` を走査し、Neo4j 側に該当する `MemoryChunk` が存在しない場合、その Qdrant ベクトルを物理削除（`delete`）する。
* **Check 2**: Neo4j 内で、どこの `(:Context)` にも属さず、かつどの `MemoryChunk` からも接続されていない孤立した `(:Item)` や `(:Artifact)` ノードを検出し、自動で `DELETE` する。

## 4. 関連情報
- ベクトル化モデルについて、`~/doc/vector-model.md`にを参照すること
- DBのURL、ベクトル化モデルのパスワードなどは環境変数で設定できるようにする。既存のコードに準拠すること。