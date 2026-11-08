#pragma once
#include "gui_types.hpp"
#include <QListWidget>
#include "photo_delegate.hpp"
namespace aigle {
inline PhotoIdentity photoIdentity(const QListWidgetItem* item) {
    return {item->data(Qt::UserRole).toString(), item->data(PhotoDelegate::CopyRole).toLongLong(), {}};
}
inline bool samePhoto(const QListWidgetItem* item, const PhotoIdentity& identity) {
    const auto current = photoIdentity(item);
    return current.path == identity.path && current.copy == identity.copy;
}
}
