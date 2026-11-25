# Độ trễ gợi ý copilot: từ hết câu nói tới lúc suggestion được publish

Trạng thái: **số đo dưới đây là chạy giả** (model giả có độ trễ mô phỏng, Redis thật, engine thật). Chúng đo khung xếp hàng và overhead của engine, KHÔNG phải độ trễ của model thật. Số của model thật do `scripts/real-provider-check.sh` (bước C) tạo ra; điền vào bảng "Model thật" bên dưới khi đã chạy.

## Cách đo

Lệnh: `warptalk-ai-eval bench-latency` (xem `docs/architecture/ai-eval.md`). Mỗi phòng phát lại cuộc họp `vi-sprint-planning` (17 segment, 9 gợi ý có kịch bản, 1 `answer`) theo đồng hồ thật, vào đúng đường của `SuggestionWorker`: `SuggestionEngine::admit` -> chờ semaphore toàn cục (8 pipeline, như `SUGGESTION_MAX_PIPELINES` của worker) -> `SuggestionEngine::run` (stage 0, decide, truy hồi tri thức, generate, hygiene, `XADD` vào stream kết quả Redis). Đồng hồ bắt đầu khi segment tới engine (tức là lúc STT chốt câu), kết thúc khi `run` trả về `Published`.

- Model giả: decide ~ N(300, 100) ms, generate ~ N(700, 200) ms, seed 1, cộng chi phí xử lý thật của fake.
- Cấu hình engine của eval (cooldown 0, tối đa 200 gợi ý/cuộc họp) nên có nhiều gợi ý hơn production; `Busy` (đã có pipeline chạy trong phòng) vẫn xảy ra như thật.
- 50 phòng cùng phát một cuộc họp, bắt đầu rải đều trong 3 s: tải đồng pha hơn thực tế (nhiều cuộc họp khác nhau sẽ ít cụm bùng nổ hơn), nên số ở mức 50 phiên thiên về xấu.
- **Không đo**: STT realtime (thường thêm 0,3 đến 1 s từ lúc hết câu tới lúc có transcript cuối), `POST /copilot/sessions/{id}/segments`, bước `stt:results` qua Redis stream (vài ms), SSE tới client, mạng tới nhà cung cấp LLM. Bench cũng chưa đi qua router HTTP của api; nó dùng cùng mã engine/worker.

## Kết quả chạy giả (tốc độ 1x, 1 cuộc họp, 8 pipeline toàn cục)

Mỗi hàng là số ms từ lúc segment tới tới lúc publish.

| phiên đồng thời | gợi ý | all p50 / p95 / p99 | `answer` p50 / p95 (n) | chờ xếp hàng p50 / p95 | thời gian LLM p50 / p95 | pipeline chờ slot tối đa |
|---|---|---|---|---|---|---|
| 1 | 9 | 1112 / 1460 / 1460 | 889 / 889 (1) | 0 / 1 | 1105 / 1454 | 1 |
| 10 | 90 | 965 / 1379 / 1605 | 958 / 1286 (10) | 0 / 0 | 961 / 1376 | 1 |
| 50 | 450 | 2310 / 3849 / 4161 | 2303 / 4016 (50) | 1344 / 2833 | 1004 / 1342 | 26 |
| 50, 24 pipeline | 450 | 1008 / 1371 / 1463 | 1045 / 1405 (50) | 0 / 0 | 1005 / 1368 | 1 |

Cùng bài ở tốc độ 8x (tải gấp 8 trên mỗi phòng, độ trễ model không co lại): 50 phiên cho `answer` p95 3753 ms, xếp hàng p50 2922 ms, 42 pipeline chờ cùng lúc. Chỉ dùng để thấy hướng, không phải số vận hành.

## Diễn giải

1. **Thời gian đi đâu**: ở tải thấp và vừa (1 và 10 phiên) toàn bộ là LLM (decide + generate, khoảng 1,0 s) cộng khoảng 4 ms overhead của engine (truy hồi, hygiene, Redis). Xếp hàng gần 0. Vậy nên với model thật, độ trễ gợi ý xấp xỉ `p95(decide) + p95(generate)`; engine không phải điểm nghẽn.
2. **Tranh chấp xuất hiện ở khoảng 50 phiên với 8 pipeline**: tổng thời gian model bận khoảng 565 s trên 141 s tức trung bình 4 pipeline, nhưng các phiên đồng pha tạo ra cụm tới 26 pipeline chờ. Xếp hàng lúc đó chiếm hơn một nửa độ trễ (p50 1,3 s trên tổng 2,3 s) và đẩy `answer` p95 lên 4,0 s. Nâng lên 24 pipeline đưa p95 về 1,4 s. Quy tắc thô: số pipeline >= (số phòng đang họp) x (số pipeline trung bình mỗi phòng, ~0,08 ở đây) x hệ số bùng nổ 3 đến 4, và còn bị giới hạn bởi rate limit của nhà cung cấp (TPM/RPM), thứ bench giả không có.
3. **`answer` không nhanh hơn loại khác**: nó đi cùng đường decide + generate nên p95 bằng các loại còn lại; hiện không có làn ưu tiên. Đây là loại người dùng đang nói chờ nhất.
4. **Ít mẫu**: ở 1 phiên chỉ có 1 mẫu `answer` nên p95 vô nghĩa; hãy đọc cột 10 và 50 phiên, hoặc thêm cuộc họp (`--meeting` lặp được, `--sessions`).

## Mục tiêu đề xuất

| chỉ tiêu | mục tiêu | ghi chú |
|---|---|---|
| `answer`: segment tới engine -> publish, p95 | **< 2,5 s** với model thật, ở tải mục tiêu | cộng STT cuối (~0,5 s) và hop mạng thì người dùng thấy < 3,5 s sau khi hết câu hỏi |
| `answer` p50 | < 1,5 s | |
| mọi category p95 | < 4 s | gợi ý chậm hơn hết hạn dần giá trị |
| thời gian xếp hàng p95 | < 200 ms ở tải mục tiêu | xếp hàng vượt ngưỡng nghĩa là thiếu pipeline, không phải model chậm |

Ngân sách 2,5 s: decide <= 0,6 s + generate <= 1,5 s + truy hồi và overhead <= 0,2 s + xếp hàng <= 0,2 s. Với model thật, decide thường 300 đến 700 ms và generate 700 đến 2000 ms; nên mục tiêu chỉ đạt được với model nhỏ/nhanh cho decide và generate (không dùng model suy luận), và đây là điều cần xác nhận bằng số thật.

## Đòn bẩy nếu số thật vượt mục tiêu

1. Cho `answer_candidate` bỏ qua hoặc gộp decide với generate (một call thay vì hai, tiết kiệm 0,3 đến 0,7 s).
2. Stream generate và publish khi đủ một câu thay vì đợi toàn bộ.
3. Làn ưu tiên cho `answer` trong semaphore toàn cục; đặt `max_concurrent_pipelines` theo số phòng đồng thời và rate limit của nhà cung cấp (đo bằng `--sessions N --max-pipelines M`).
4. Model nhỏ hơn cho decide (đã tách provider/model theo `Purpose`).
5. Tránh cold start: giữ kết nối HTTP/2 tới nhà cung cấp, làm nóng cache prompt (system prompt ổn định ở đầu request).

## Model thật (điền sau khi chạy `scripts/real-provider-check.sh`)

| ngày | provider / model | phiên | `answer` p50 / p95 (n) | all p95 | xếp hàng p95 | thời gian LLM p95 |
|---|---|---|---|---|---|---|
| chưa chạy | | | | | | |

## Chạy lại

```
cargo run -p warptalk-ai-eval -- bench-latency --meeting vi-sprint-planning --speed 1 \
  --sessions 1,10,50 --out out/bench.json --md-out out/bench.md       # ~8 phút (3 nấc x ~2,4 phút)
cargo run -p warptalk-ai-eval -- bench-latency --meeting vi-sprint-planning --speed 1 \
  --sessions 50 --max-pipelines 24                                      # kiểm tra số pipeline
```

Kết quả JSON có `levels[].{total,answer,queue,llm,other}` với p50/p95/p99.
