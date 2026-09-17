<?php
declare(strict_types=1);

$path = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH);

if ($path === '/__coffeepos_runtime_health') {
    header('Content-Type: application/json; charset=utf-8');
    header('Cache-Control: no-store');
    echo json_encode([
        'service' => 'coffeepos-development-runtime',
        'status' => 'ready',
        'php_version' => PHP_VERSION,
    ], JSON_UNESCAPED_SLASHES);
    return;
}

http_response_code(404);
header('Content-Type: text/plain; charset=utf-8');
echo "CoffeePOS runtime readiness fixture\n";
