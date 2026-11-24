# Đánh giá offline lõi AI (`warptalk-ai-eval`)

Công cụ đo chất lượng gợi ý trực tiếp (SuggestionEngine) và tóm tắt sau họp (SummaryService) trên các cuộc họp tổng hợp có nhãn, để thay đổi prompt/model có số liệu thay vì cảm tính. Mã nguồn: `apps/ai-eval`.

## Cách hoạt động

- Mỗi cuộc họp trong `apps/ai-eval/datasets/*.json` được phát lại từng dòng qua `SuggestionEngine::process` (cùng đường `admit` + `run` mà `SuggestionWorker` dùng). Cổng/consent/tín hiệu/kho tri thức là bản trong bộ nhớ; riêng trạng thái slot, dedup và stream kết quả của engine nằm trong Redis (key `evalsuggest:*`, xóa sau mỗi cuộc họp). Cooldown tắt và `max_per_meeting=200` vì phát lại nhanh hơn thời gian thực; timeout mỗi call 30 giây.
- Tóm tắt chạy qua `SummaryService::on_meeting_end` rồi `run` với nguồn transcript/phòng trong bộ nhớ; dòng job trong Postgres bị xóa sau khi đọc.
- Trường `channel` (`me|others|unknown`) của từng dòng được đưa vào `SttResult.channel`, nên vai trò self/other và category `answer` được đo như khi chạy thật.

## Chạy

Cần Redis (`REDIS_URL`, mặc định `redis://localhost:6379`) và Postgres (`DATABASE_URL`, mặc định `postgres://postgres@localhost:5432/warptalk`; tự chạy migration settings + assistant).

```
# CI / không cần key: provider giả, kết quả xác định
cargo run -p warptalk-ai-eval -- run --suite all --out /tmp/fake.json

# Model thật (key lấy từ môi trường, không bao giờ ghi log)
export OPENAI_API_KEY=...        # hoặc ANTHROPIC_API_KEY / GEMINI_API_KEY
cargo run -p warptalk-ai-eval -- run --suite all --provider openai --model gpt-4.1-mini \
  --judge-provider openai --judge-model gpt-4.1 --out out/gpt41mini.json \
  --compare apps/ai-eval/baselines/fake.json

# In lại bảng markdown (có cột chênh lệch so với baseline)
cargo run -p warptalk-ai-eval -- report --in out/gpt41mini.json --compare out/before.json
```

Tùy chọn khác: `--meeting ID` (lặp được), `--dataset-dir`, `--tolerance N`, `--suggestion-config f.json` / `--summary-config f.json` (ghi đè cấu hình, ví dụ `prompt_templates`, `sensitivity`, `window_size`), `--price-in/--price-out` (USD/1M token). Mã thoát: 0 ổn, 1 hồi quy so với baseline, 2 lỗi.

## Thêm một cuộc họp

1. Tạo `apps/ai-eval/datasets/<id>.json` theo định dạng trong doc comment của `src/dataset.rs`: `lines` (`t_ms`, `channel`, `speaker`, `text`; text mỗi dòng phải duy nhất), `snippets` (tài liệu tham chiếu cho kho tri thức giả), `expectations`.
2. `at_line` đánh số từ 0. `category` có thể liệt kê phương án thay thế (`fact|answer`); `action` được coi là `action_item`. `must_include` hỗ trợ `a|b` (một trong hai); so khớp không phân biệt hoa thường và dấu tiếng Việt.
3. Đặt bẫy: số liệu không có trong nguồn (`must_not_include`), câu hỏi hướng tới "me", thuật ngữ cần định nghĩa, owner/deadline chỉ khi có nói (action item không có `owner`/`deadline` nghĩa là transcript không nêu; model bịa ra sẽ bị tính sai).
4. Khối `fake` (tùy chọn) là kịch bản cho provider giả: `suggestions[{at_line, category, content, ...}]` và `summary` (đầu ra thô của model). Dùng toàn dữ liệu và bẫy cố ý để baseline có điểm không hoàn hảo.
5. Chạy lại `run` với provider giả và cập nhật `baselines/fake.json` khi thay đổi có chủ đích.

Dữ liệu hiện có: 4 cuộc họp tiếng Việt (sprint planning pha tiếng Anh, cuộc gọi bán hàng, 1:1, review sự cố) và 2 tiếng Anh. Toàn bộ là tổng hợp, không có người hay công ty thật.

## Định nghĩa chỉ số

Gợi ý (gộp trên mọi cuộc họp):
- Một gợi ý **đúng** khi khớp một expectation chưa dùng: cùng category, xuất hiện tại `at_line` đến `at_line + tolerance`, thỏa đủ `must_include` và không chứa `must_not_include`. Khớp vị trí nhưng sai nội dung là "wrong content" (không tính đúng).
- **precision** = đúng / số gợi ý đã phát; **recall** = đúng / số expectation; **F1**. Precision chỉ so với phần đã gán nhãn nên là cận dưới (gợi ý hợp lý nhưng chưa gán nhãn bị tính là sai).
- **forbidden-hit rate** = số mục `forbidden` bị phát đúng dòng, đúng category / tổng mục forbidden (thấp hơn là tốt).
- **hallucination rate** = số gợi ý chứa cụm bẫy (bẫy toàn cục + `must_not_include` của expectation phủ dòng đó) / số gợi ý đã phát (thấp hơn là tốt).
- Theo category (khóa là nhãn của expectation), độ trễ p50/p95 theo loại call (decide, generate), token và chi phí ước tính (bảng giá tham khảo trong `src/cost.rs`, có thể lỗi thời), histogram outcome của engine.

Tóm tắt:
- **decision recall**: decision kỳ vọng có ít nhất một decision của model khớp (khớp từ khóa: tối thiểu một nửa số từ khóa, làm tròn lên).
- **action recall / precision**: ghép action item kỳ vọng với item của model theo `task_keywords`.
- **owner / deadline accuracy** trên các item đã ghép: có kỳ vọng thì phải khớp; không có kỳ vọng thì model phải để trống (bịa owner/deadline bị tính sai và đếm vào `owner_invented`/`deadline_invented`).
- **trap-hit rate** = số cụm trong `must_not_include` xuất hiện trong toàn bộ bản tóm tắt / tổng số cụm.
- **Judge** (tùy chọn): model chấm faithfulness 1 đến 5 cho từng gợi ý / từng bản tóm tắt; báo cáo trung bình. Token của judge không tính vào chi phí.

Provider `fake` trả lời theo kịch bản và tôn trọng danh sách category mà request cho phép; độ trễ là giá trị mô phỏng (theo số token đầu ra). Baseline `baselines/fake.json` chủ ý không hoàn hảo.

## Khi đổi prompt hoặc model

1. Chạy trên baseline cũ (checkout trước khi đổi) với model thật, lưu `before.json`.
2. Đổi prompt/model/cấu hình, chạy lại với `--compare before.json` (ngưỡng mặc định tụt tuyệt đối 0,03 mỗi chỉ số chất lượng; độ trễ và chi phí chỉ báo cáo, không chặn).
3. Xem các expectation hụt (in cuối báo cáo) và `hallucinations` / `forbidden_hits` trong JSON thay vì chỉ nhìn điểm tổng; mẫu nhỏ (6 cuộc họp) nên chênh dưới vài điểm phần trăm là nhiễu, chạy lại vài lần với model thật.
4. `cargo test -p warptalk-ai-eval` chạy bộ fake với engine thật và so với `baselines/fake.json`: nó phát hiện hỏng harness hoặc thay đổi hành vi engine/prompt làm lệch điểm của kịch bản cố định. Khi đổi category hoặc prompt có chủ đích, chạy lại fake và commit baseline mới.
