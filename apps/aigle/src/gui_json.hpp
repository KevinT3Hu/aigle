#pragma once
#include "photo_json.hpp"
#include "gui_types.hpp"
#include <QJsonArray>
#include <QJsonObject>

namespace aigle {
inline ExportProgress exportProgressFromJson(const QJsonObject& object) {
    ExportProgress result;
    result.jobId = object["job_id"].toString();
    result.running = object["running"].toBool();
    result.hasResult = object.contains("result");
    result.cancelled = object.contains("cancelled");
    if (object.contains("error")) result.error = object["error"].toString();
    return result;
}
inline QList<double> numbersFromJson(const QJsonArray& array) {
    QList<double> result;
    for (const auto& value : array) result.append(value.toDouble());
    return result;
}
inline Histogram histogramFromJson(const QJsonObject& object) {
    return {!object.isEmpty(), numbersFromJson(object["red"].toArray()),
        numbersFromJson(object["green"].toArray()), numbersFromJson(object["blue"].toArray()),
        numbersFromJson(object["luma"].toArray())};
}
inline ImageInfo imageInfoFromJson(const QJsonObject& object) {
    ImageInfo result;
    result.kind = object["kind"].toString();
    const auto wb = object["as_shot"].toObject();
    if (wb["temperature"].isDouble()) result.temperature = wb["temperature"].toDouble();
    if (wb["tint"].isDouble()) result.tint = wb["tint"].toDouble();
    return result;
}
inline CurvePoints curvePointsFromJson(const QJsonArray& array) {
    CurvePoints result;
    for (const auto& value : array) {
        const auto point = value.toObject();
        result.append({point["x"].toDouble(), point["y"].toDouble()});
    }
    return result;
}
inline QJsonArray toJson(const CurvePoints& points) {
    QJsonArray result;
    for (const auto& p : points) result.append(QJsonObject{{"x", p.x()}, {"y", p.y()}});
    return result;
}
inline ScopeResult scopeResultFromJson(const QJsonObject& object) {
    ScopeResult result;
    result.requestId = object["request_id"].toString();
    if (object.contains("error")) result.error = object["error"].toString();
    result.available = object["available"].toBool();
    result.diffuseWhite = object["diffuse_white"].toDouble();
    for (const auto& value : object["targets"].toArray()) {
        const auto point = value.toArray();
        result.targets.append({point.at(0).toDouble(), point.at(1).toDouble()});
    }
    const auto data = object["data"].toObject();
    result.data.available = !data.isEmpty();
    result.data.hdr = data["hdr"].toBool();
    const auto bins = data["bins"].toArray(), wave = data["wave"].toArray();
    for (int c = 0; c < 4; ++c) {
        result.data.bins[size_t(c)] = numbersFromJson(bins.at(c).toArray());
        result.data.wave[size_t(c)] = numbersFromJson(wave.at(c).toArray());
    }
    result.data.vector = numbersFromJson(data["vector"].toArray());
    result.data.count = data["count"].toDouble();
    result.data.low = data["low"].toDouble();
    result.data.high = data["high"].toDouble();
    result.data.gamut = data["gamut"].toDouble();
    const QSize size(object["width"].toInt(), object["height"].toInt());
    if (size.width() > 0 && size.height() > 0 && size.width() <= 128 && size.height() <= 128) {
        result.thumbnail = QImage(size, QImage::Format_RGBA8888);
        result.thumbnail.fill(Qt::transparent);
        const auto pixels = object["thumbnail"].toArray();
        for (qsizetype i = 0; i < std::min<qsizetype>(pixels.size(), size.width() * size.height()); ++i) {
            const auto pixel = pixels.at(i).toArray();
            auto* out = result.thumbnail.scanLine(int(i / size.width())) + int(i % size.width()) * 4;
            for (int c = 0; c < 4; ++c) out[c] = uchar(pixel.at(c).toInt());
        }
    }
    return result;
}
inline QJsonValue toJson(const DisplayProfile& profile) {
    switch (profile.kind) {
    case DisplayProfile::Kind::Icc: return QJsonObject{{"Icc", profile.icc}};
    case DisplayProfile::Kind::HdrRec2020: return "HdrRec2020";
    case DisplayProfile::Kind::Srgb: return "Srgb";
    }
    return "Srgb";
}
inline DisplayDetection displayDetectionFromJson(const QJsonObject& value) {
    DisplayDetection result;
    result.running = value["running"].toBool();
    result.screen = value["screen"].toString();
    result.name = value["name"].toString();
    result.error = value["error"].toString();
    const auto profile = value["profile"].toObject();
    if (profile["Icc"].isString())
        result.profile = DisplayProfile{DisplayProfile::Kind::Icc, profile["Icc"].toString()};
    return result;
}
inline QJsonObject viewportJson(const ViewportRequest& value) {
    const auto& r = value.region;
    return {{"region", QJsonObject{{"x", r.x()}, {"y", r.y()}, {"w", r.width()}, {"h", r.height()}}},
            {"output_size", QJsonArray{value.outputSize.width(), value.outputSize.height()}},
            {"display", toJson(value.display)}};
}
inline QJsonObject toJson(const ViewportRequest& value) {
    return {{"cmd", "viewport"}, {"continuous", value.continuous}, {"viewport", viewportJson(value)}};
}
inline QJsonObject toJson(const RetouchDefaults& value) {
    const auto tool = value.tool == RetouchDefaults::Tool::RedEye ? "red_eye"
                    : value.tool == RetouchDefaults::Tool::Clone ? "clone" : "heal";
    return {{"tool", tool}, {"radius", value.radius}, {"feather", value.feather},
            {"opacity", value.opacity}, {"pupil", value.pupil}, {"darken", value.darken}};
}
}
