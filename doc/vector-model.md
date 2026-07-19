# ベクトル化モデルの使用方法
以下のサンプルプログラムのように利用することができる。
``` python
import requests, json, html
import urllib3

# 接続設定
url = "https://aisvr221.aikb.kyutech.ac.jp/api/embeddings"
headers = {
    "Content-Type": "application/json",
    "Authorization": "password"  # ← 自分のPW
}
payload = {
    "model": "jeffh/intfloat-multilingual-e5-large:f32",
    "prompt": "カレーの作り方",
    "stream": True
}

r = requests.post(url, headers=headers, json=payload, verify=False, timeout=30)
print("REQUEST URL:", r.request.url)
print("Status:", r.status_code)
print("WWW-Authenticate:", r.headers.get("WWW-Authenticate"))
```

使用可能なモデルは以下の通り
<embedding model>
jeffh/intfloat-multilingual-e5-large:f32
jeffh/intfloat-multilingual-e5-large:f16
jeffh/intfloat-multilingual-e5-large:q8_0
zylonai/multilingual-e5-large:latest
mxbai-embed-large-embed:latest
mxbai-embed-large:latest
mxbai-embed-large:335m
