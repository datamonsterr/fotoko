import { useCallback, useRef, useState } from 'react';
import { randomUUID } from 'expo-crypto';
import * as ImagePicker from 'expo-image-picker';

import { useI18n } from '@/i18n';
import { MAX_PHOTOS } from '@/lib/limits';
import { readBlobSize, toDraftPhotos } from '@/lib/photos';
import { usePhotoDraft } from '@/state/draft';

/**
 * Opens the system photo library and adds the result to the draft. `picking` is true from the
 * tap until photos are added (disable the button: the picker and the size reads are slow);
 * a picker or read failure becomes a notice instead of an unhandled rejection.
 */
export function usePhotoPicker(): {
  pick: () => Promise<void>;
  picking: boolean;
  notices: string[];
} {
  const { t } = useI18n();
  const { photos, dispatch } = usePhotoDraft();
  const [picking, setPicking] = useState(false);
  const [notices, setNotices] = useState<string[]>([]);
  const busy = useRef(false);

  const pick = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    setPicking(true);
    try {
      const res = await ImagePicker.launchImageLibraryAsync({
        mediaTypes: ['images'],
        allowsMultipleSelection: true,
        selectionLimit: MAX_PHOTOS - photos.length,
        quality: 1,
      });
      if (res.canceled) return;
      const converted = await toDraftPhotos(res.assets, {
        existingCount: photos.length,
        newId: randomUUID,
        readSize: readBlobSize,
      });
      dispatch({ type: 'add', photos: converted.photos });
      const next: string[] = [];
      if (converted.overLimit > 0)
        next.push(t('picker.limit', { max: MAX_PHOTOS }));
      if (converted.tooBig > 0) next.push(t('picker.tooBig'));
      if (converted.unsupported > 0) next.push(t('picker.unsupported'));
      setNotices(next);
    } catch {
      setNotices([t('picker.failed')]);
    } finally {
      busy.current = false;
      setPicking(false);
    }
  }, [photos.length, dispatch, t]);

  return { pick, picking, notices };
}
