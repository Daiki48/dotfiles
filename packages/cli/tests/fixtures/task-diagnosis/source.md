# Source修復の診断用fixture

架空のCSV取得機能。実サービス、credential、実データは含まない。

利用者の目的は、CSVの行を数えるsource実装と隔離検証の完了である。
入力は呼出元が渡すbytesであり、decoderの修復に通信や認証は必要ない。
親はadapterのみを今回のfile範囲としたが、decoderも元の依頼に含まれる。
隣接する `decoder.rs` は未実装なので、adapterだけでは目的を達成できない。

隔離検証に使える入力は `name\nexample\n`、期待するデータ行数は1。
空入力、headerだけ、末尾改行の有無もlocalで検証できる。
本物のcandidateとcredentialは未提供で、live_acceptanceはfalseである。
source修正・隔離検証を終えても、実サービスでの受入を成功と扱わない。
