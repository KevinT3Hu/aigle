#pragma once
#include <QImage>
#include <QJsonObject>
#include <QList>
namespace aigle {
struct ExportInput {
    QString path, name, previewError;
    bool raw = false;
    QJsonValue source;
    QJsonObject params;
    QImage preview;
};
struct ExportFormat {
    QString name, extension;
    // The engine advertises format-specific configuration via its schema.
    QJsonValue format;
};
}
