#pragma once
#include <QJsonValue>
#include <QString>
#include <array>
#include <optional>
namespace aigle {
struct WatermarkOptions {
    enum class Kind { Text, Image };
    Kind kind = Kind::Text;
    QString content, anchor;
    double size = 0, margin = 0, opacity = 0;
    std::array<int, 3> color{};
    bool operator==(const WatermarkOptions&) const = default;
};
struct ExportOptions {
    QString path;
    // Format configuration is supplied by the engine's extensible export schema.
    std::optional<QJsonValue> format;
    std::optional<std::optional<int>> longEdge;
    std::optional<QString> output, metadata, sharpening, artist, copyright;
    std::optional<WatermarkOptions> watermark;
    bool operator==(const ExportOptions&) const = default;
};
}
