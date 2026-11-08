#pragma once
#include <QString>
#include <optional>
namespace aigle {
struct EngineConfig {
    QString dataDirectory;
    enum class Backend { Auto, Cpu, Gpu };
    Backend backend = Backend::Auto;
    bool deterministic = false;
    int overviewLongEdge = 1536;
    std::optional<int> workerThreads = std::nullopt;
    std::optional<quint64> memoryBudget = std::nullopt;
    std::optional<QString> cacheDirectory = std::nullopt;
};
}
