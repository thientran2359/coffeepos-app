# Provisioning — kế hoạch Phase 3–4

Chưa thực thi trong Phase 1. PHP, database và WordPress không được tải/copy từ LocalWP hiện có.

1. Kiểm manifest và đủ dung lượng, giữ installation lock.
2. Ensure data directories; journal tiến độ không chứa secret.
3. Chỉ init MariaDB khi xác nhận chưa có datadir. Partial/corrupt datadir phải có recovery riêng, không xóa rồi init lại.
4. Start DB, ensure database/account least privilege; root chỉ phục vụ bootstrap.
5. Stage template versioned, generate wp-config ngoài log, quyền file hạn chế.
6. Cài WordPress qua bootstrap CLI/script được đóng gói; đảm bảo chưa installed trước khi install.
7. Ensure WooCommerce và CoffeePOS installed/active; plugin tự sở hữu migrations và initialization.
8. Ensure initial admin theo explicit setup; retry không đổi mật khẩu admin hiện có.
9. Health thành công mới đánh dấu installation ready và mở canonical POS URL do plugin cung cấp.

Uploads được cấu hình vào directory mutable đã quy định. Template chứa WP/WC/CoffeePOS đã pin, không chứa store database/uploads hoặc credentials. Mỗi bước phải có fresh/retry/existing-store tests.

## Health API đề xuất — cần align plugin trước khi code

`GET /wp-json/coffeepos/v1/system/status`

```json
{
  "schema_version": 1,
  "wordpress": true,
  "woocommerce": true,
  "coffeepos": true,
  "database": true,
  "store": { "name": "My Coffee" },
  "pos_path": "/pos/"
}
```

`pos_path` là ví dụ, plugin phải sinh từ router thật, desktop resolve tương đối theo origin đã kiểm chứng. Không hardcode POS path như domain contract.

HTTP 200 chỉ khi required components ready, 503 khi endpoint còn chạy nhưng dependency lỗi, 401 khi thiếu/sai machine credential. Nếu PHP/WP/DB lỗi tới mức endpoint không chạy hoặc plugin bị inactive thì desktop phân loại transport/bootstrap failure; không giả health OK.

Request dùng header machine token riêng, so sánh constant-time trong plugin, token truyền qua native HTTP client và giữ trong protected storage; không đưa token vào WebView, query string hoặc log. Không dùng quyền admin để auth health. Khi LAN bật, endpoint phải có auth dù caller cùng LAN; không public store info. Response không chứa secret, filesystem paths hay dữ liệu khách.

Schema/token bootstrap/rotation/version response cần chốt cùng plugin trước Phase 4. Desktop đọc kết quả endpoint, không tái tạo business/dependency logic WordPress trong Rust.
