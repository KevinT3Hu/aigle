#pragma once
#include <QString>
#include <optional>
namespace aigle {
struct ExportProgress {
    QString jobId;
    bool running = false, hasResult = false, cancelled = false;
    std::optional<QString> error;
};
}
