#pragma once
#include "export_input.hpp"
#include <QJsonArray>
namespace aigle {
inline QList<ExportFormat> exportFormatsFromJson(const QJsonArray& values) {
    QList<ExportFormat> result;
    for (const auto& value : values) {
        const auto object = value.toObject();
        result.append({object["name"].toString(), object["extension"].toString(), object["format"]});
    }
    return result;
}
inline QList<ExportInput> exportInputsFromJson(const QJsonArray& values) {
    QList<ExportInput> result;
    for (const auto& value : values) {
        const auto object = value.toObject();
        ExportInput input;
        input.path = object["path"].toString();
        input.name = object["name"].toString();
        input.raw = object["raw"].toBool();
        input.source = object["source_ref"];
        input.params = object["params"].toObject();
        input.previewError = object["preview_error"].toString();
        const auto preview = object["preview"].toObject();
        const auto bytes = QByteArray::fromBase64(preview["bgra"].toString().toLatin1());
        const int w = preview["width"].toInt(), h = preview["height"].toInt(), stride = preview["stride"].toInt();
        if (w > 0 && w <= 388 && h > 0 && h <= 388 && stride >= w * 4 && qint64(stride) * h <= bytes.size())
            input.preview = QImage(reinterpret_cast<const uchar*>(bytes.constData()), w, h, stride, QImage::Format_ARGB32).copy();
        result.append(input);
    }
    return result;
}
}
