#pragma once
#include <QMap>
#include <QStringList>
namespace aigle {
struct PhotoIdentity {
    QString path;
    qint64 copy = 0;
    QString name;
    bool operator==(const PhotoIdentity&) const = default;
};
struct SelectionPhotoMetadata {
    bool loaded = false;
    QStringList tags;
    QMap<QString, QString> iptc;
};


}
