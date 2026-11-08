#pragma once
#include <QCoreApplication>
#include <QApplication>
#include <algorithm>
#include <QPainter>
#include <QStyledItemDelegate>
namespace aigle {
// One shared delegate keeps fit-inside thumbnails and their selection/metadata
// boundaries identical in Library and the filmstrip. Never crop the photograph.
class PhotoDelegate final : public QStyledItemDelegate {
    Q_DECLARE_TR_FUNCTIONS(aigle::PhotoDelegate)
  public:
    enum { RatingRole = Qt::UserRole + 1, CopyRole, ImageRole, MetadataRole, StackCountRole };
    explicit PhotoDelegate(QObject* parent = nullptr) : QStyledItemDelegate(parent) {}
    bool film = false;
    QSize cell{285, 216};
    QSize sizeHint(const QStyleOptionViewItem&, const QModelIndex&) const override { return cell; }
    void paint(QPainter* p, const QStyleOptionViewItem& option,
               const QModelIndex& index) const override {
        p->save();
        p->setRenderHint(QPainter::Antialiasing);
        const bool selected = option.state & QStyle::State_Selected;
        const QRect box = option.rect.adjusted(4, 4, -4, -4);
        p->setPen(selected ? QColor("#8aaed4") : QColor("#242424"));
        p->setBrush(selected ? QColor("#2e3339") : QColor("#242424"));
        p->drawRoundedRect(box.adjusted(0, 0, -1, -1), 2, 2);
        const QRect picture =
            box.adjusted(film ? 4 : 8, film ? 4 : 8, film ? -4 : -8, film ? -17 : -43);
        p->fillRect(picture, QColor("#1b1b1b"));
        const auto image = qvariant_cast<QImage>(index.data(ImageRole));
        if (!image.isNull()) {
            const auto fitted = image.size().scaled(picture.size(), Qt::KeepAspectRatio);
            p->drawImage(
                QRect(picture.center() - QPoint(fitted.width() / 2, fitted.height() / 2), fitted),
                image);
        }
        auto font = QApplication::font();
        font.setPixelSize(film ? 10 : 11);
        p->setFont(font);
        p->setPen(QColor("#ddd"));
        if (!film) {
            const QRect caption(box.x() + 8, picture.bottom() + 7, box.width() - 16, 16);
            p->drawText(caption, Qt::AlignLeft | Qt::AlignVCenter,
                        p->fontMetrics().elidedText(index.data().toString(), Qt::ElideMiddle,
                                                    caption.width()));
        }
        if (index.data(CopyRole).toLongLong()) {
            const QRect badge(picture.right() - 23, picture.top() + 4, 20, 20);
            p->fillRect(badge, QColor(25, 25, 25, 210));
            p->setPen(QColor("#ddd"));
            p->setBrush(Qt::NoBrush);
            p->drawRect(badge.adjusted(4, 3, -6, -6));
            p->drawRect(badge.adjusted(7, 6, -3, -3));
        }
        const int rating = std::clamp(index.data(RatingRole).toInt(), 0, 5);
        p->setPen(QColor("#b0b0b0"));
        const QRect bottom(box.x() + 8, box.bottom() - 17, box.width() - 16, 14);
        p->drawText(bottom, (film ? Qt::AlignHCenter : Qt::AlignLeft) | Qt::AlignVCenter,
                    QString(rating, QChar(0x2605)) +
                        (film ? QString{} : QString(5 - rating, QChar(0x2606))));
        const int stackSize = index.data(StackCountRole).toInt();
        if (!film && stackSize > 0)
            p->drawText(bottom, Qt::AlignRight | Qt::AlignVCenter,
                        tr("Stack · %1").arg(stackSize));
        if (!film && selected) {
            const QRect check(picture.x() + 5, picture.y() + 5, 17, 17);
            p->setBrush(QColor("#dce8f5"));
            p->setPen(Qt::NoPen);
            p->drawRoundedRect(check, 2, 2);
            p->setPen(QColor("#1e2731"));
            p->drawText(check, Qt::AlignCenter, QString::fromUtf8("✓"));
        }
        if (option.state & QStyle::State_HasFocus) {
            p->setPen(QPen(QColor("#8aaed4"), 1, Qt::DotLine));
            p->setBrush(Qt::NoBrush);
            p->drawRect(box.adjusted(2, 2, -3, -3));
        }
        p->restore();
    }
};
} // namespace aigle
